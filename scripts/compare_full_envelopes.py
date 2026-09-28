#!/usr/bin/env python3
"""Compare complete native C ABI envelopes in separate profile processes."""
from __future__ import annotations
import argparse
import copy
import ctypes
import datetime as dt
import difflib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False)+'\n')


def normalize(value):
    result = copy.deepcopy(value)
    if isinstance(result.get('result'), dict):
        result['result'].pop('duration', None)
        metadata = result['result'].get('frontmatter')
        if isinstance(metadata, dict):
            metadata.pop('markitai_processed', None)
    return result


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False)


def differing_paths(left, right, path='$'):
    if type(left) is not type(right):
        return [path]
    if isinstance(left, dict):
        output=[]
        for key in sorted(left.keys() | right.keys()):
            if key not in left or key not in right:
                output.append(path+'.'+key)
            else:
                output.extend(differing_paths(left[key],right[key],path+'.'+key))
        return output
    if isinstance(left, list):
        if len(left)!=len(right):
            return [path]
        return [item for index,(a,b) in enumerate(zip(left,right)) for item in differing_paths(a,b,f'{path}[{index}]')]
    return [] if left==right else [path]


def worker(manifest_path, profile):
    manifest=json.loads(manifest_path.read_text())
    root=manifest_path.parent
    dest=root/profile
    identity=manifest['profiles'][profile]
    library_path=Path(identity['frozen_library'])
    if digest(library_path)!=identity['sha256']:
        raise RuntimeError('Frozen library hash mismatch before loading')
    # This audits Python path/network events only. Explicit conversion options
    # isolate native execution; this is not an OS sandbox for native syscalls.
    spec=importlib.util.spec_from_file_location('markitai_benchmark_guards',manifest['guard_module'])
    module=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    guards=module.install_python_guards()
    class Buffer(ctypes.Structure):
        _fields_=[('data',ctypes.POINTER(ctypes.c_ubyte)),('len',ctypes.c_size_t)]
    library=ctypes.CDLL(str(library_path))
    library.markitai_abi_version.argtypes=[]
    library.markitai_abi_version.restype=ctypes.c_uint32
    if library.markitai_abi_version()!=1:
        raise RuntimeError('Unknown native ABI version')
    library.markitai_version.argtypes=[]
    library.markitai_version.restype=ctypes.c_char_p
    library.markitai_convert_json.argtypes=[ctypes.POINTER(ctypes.c_ubyte),ctypes.c_size_t]
    library.markitai_convert_json.restype=Buffer
    library.markitai_buffer_free.argtypes=[ctypes.POINTER(Buffer)]
    library.markitai_buffer_free.restype=None
    records=[]
    for case in manifest['cases']:
        source=Path(case['source'])
        if digest(source)!=case['input_sha256']:
            raise RuntimeError('Input changed before conversion: '+str(source))
        encoded=case['request_json'].encode('utf-8')
        allocation=(ctypes.c_ubyte*len(encoded)).from_buffer_copy(encoded)
        response=library.markitai_convert_json(allocation,len(encoded))
        try:
            if not response.data or response.len>256*1024*1024:
                raise RuntimeError('Invalid native response buffer')
            raw=ctypes.string_at(response.data,response.len)
        finally:
            library.markitai_buffer_free(ctypes.byref(response))
        if response.data or response.len:
            raise RuntimeError('Native buffer ownership was not cleared')
        envelope=json.loads(raw)
        if not isinstance(envelope,dict) or type(envelope.get('ok')) is not bool:
            raise RuntimeError('Invalid native envelope')
        if envelope['ok'] and not isinstance(envelope.get('result'),dict):
            raise RuntimeError('Missing native result')
        if not envelope['ok'] and not isinstance(envelope.get('error'),dict):
            raise RuntimeError('Missing native error')
        if envelope['ok'] and (type(envelope['result'].get('duration')) not in (int,float)):
            raise RuntimeError('Native duration is not a number')
        (dest/(case['id']+'.json')).write_bytes(raw)
        if digest(source)!=case['input_sha256']:
            raise RuntimeError('Input changed during conversion: '+str(source))
        records.append({'id':case['id'],'ok':envelope['ok'],'response_sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw)})
    if guards['blocked_state_events'] or guards['blocked_network_events']:
        raise RuntimeError('Worker attempted protected Python state/network access')
    if digest(library_path)!=identity['sha256']:
        raise RuntimeError('Frozen library changed during conversion')
    summary={'profile':profile,'pid':os.getpid(),'library':identity,'version':library.markitai_version().decode(),
             'cases':len(records),'successes':sum(x['ok'] for x in records),'errors':sum(not x['ok'] for x in records),
             'raw_responses':records,'python_isolation':guards,'state_directory':os.environ['MARKITAI_HOME']}
    write_json(dest/'summary.json',summary)
    return 0


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference',type=Path)
    parser.add_argument('--project-root',type=Path,default=Path.cwd(),help='Rust repository root containing scripts/benchmark_api.py; defaults to the caller working directory')
    parser.add_argument('--release-library',type=Path)
    parser.add_argument('--release-sha256')
    parser.add_argument('--dist-library',type=Path)
    parser.add_argument('--dist-sha256')
    parser.add_argument('--source-revision',help='Coordinator-verified source revision shared by both profile builds')
    parser.add_argument('--output',type=Path)
    parser.add_argument('--timeout',type=int,default=240)
    parser.add_argument('--worker',choices=['release','dist'])
    parser.add_argument('--manifest',type=Path)
    args=parser.parse_args()
    if args.worker:
        return worker(args.manifest,args.worker)
    if not all([args.reference,args.release_library,args.release_sha256,args.dist_library,args.dist_sha256,args.source_revision,args.output]):
        parser.error('Reference, both libraries/hashes, source revision, and output are required')
    root=args.output.resolve(); reference=args.reference.resolve(); repo=args.project_root.resolve()
    guard_module=repo/'scripts/benchmark_api.py'
    if not guard_module.is_file(): parser.error('Project root must contain scripts/benchmark_api.py')
    if root==reference or reference in root.parents:
        parser.error('Output must be outside the reference checkout')
    if root.exists() and any(root.iterdir()):
        parser.error('Output directory must be empty')
    formats=reference/'packages/markitai/tests/fixtures'
    format_sources=sorted(p for p in formats.glob('sample.*') if p.suffix!='.urls')
    format_sources+=sorted(p for p in (formats/'legacy').glob('*') if p.suffix in {'.doc','.ppt','.xls'})
    html=reference/'packages/markitai/tests/defuddle_fixtures'
    html_sources=sorted(p for p in (html/'fixtures').glob('*.html') if (html/'expected'/(p.stem+'.md')).is_file())
    if len(format_sources)!=24 or len(html_sources)!=209:
        raise RuntimeError(f'Expected full 24+209 corpus, discovered {len(format_sources)}+{len(html_sources)}')
    cfg={'llm':{'enabled':False},'ocr':{'enabled':False},'screenshot':{'enabled':False},
         'image':{'alt_enabled':False,'desc_enabled':False},'cache':{'enabled':False},'history':{'record':False}}
    options={'config':cfg,'llm':False,'ocr':False,'screenshot':False,'alt':False,'desc':False}
    cases=[]
    for group,paths,base in [('format',format_sources,formats),('html',html_sources,html/'fixtures')]:
        for index,path in enumerate(paths):
            case={'id':f'{group}-{index:03d}','group':group,'name':str(path.relative_to(base)),
                  'source':str(path),'input_sha256':digest(path),
                  'request_json':json.dumps({'source':str(path),'options':options},ensure_ascii=False,separators=(',',':'))}
            if group=='html': case['expected_sha256']=digest(html/'expected'/(path.stem+'.md'))
            cases.append(case)
    root.mkdir(parents=True,exist_ok=True)
    profiles={}
    for profile,lib,expected_hash in [('release',args.release_library,args.release_sha256),('dist',args.dist_library,args.dist_sha256)]:
        lib=lib.resolve()
        if digest(lib)!=expected_hash: raise RuntimeError('Coordinator library hash mismatch: '+profile)
        dest=root/profile; dest.mkdir(); state=dest/'state'; state.mkdir(); (state/'tmp').mkdir()
        frozen=dest/lib.name; shutil.copyfile(lib,frozen)
        if digest(frozen)!=expected_hash: raise RuntimeError('Frozen hash mismatch: '+profile)
        profiles[profile]={'origin':str(lib),'frozen_library':str(frozen),'sha256':expected_hash,'bytes':frozen.stat().st_size}
    manifest={'schema':1,'generated_at':dt.datetime.now(dt.timezone.utc).isoformat(),'source_revision':args.source_revision,
              'scope':'Same requests, complete C ABI JSON envelopes, separate process for each profile; correctness only.',
              'normalization':['result.duration','result.frontmatter.markitai_processed'],
              'feature_scope':'Local input; output_dir omitted; LLM, OCR, screenshots, alt, descriptions, cache and history disabled. Complements disk asset audits; does not exercise enabled remote/provider/browser behavior.',
              'profiles':profiles,'guard_module':str(guard_module),'guard_module_sha256':digest(guard_module),
              'helper':{'path':str(Path(__file__).resolve()),'sha256':digest(Path(__file__))},'counts':{'formats':24,'html':209,'total':233},'cases':cases}
    manifest_path=root/'manifest.json'; write_json(manifest_path,manifest)
    for profile in ['release','dist']:
        dest=root/profile; state=dest/'state'; config_path=state/'config.json'; write_json(config_path,cfg)
        env={'PATH':os.defpath,'LANG':'C.UTF-8','LC_ALL':'C.UTF-8','PYTHONDONTWRITEBYTECODE':'1','PYTHONNOUSERSITE':'1',
             'PYTHON_DOTENV_DISABLED':'1','MARKITAI_HOME':str(state),'MARKITAI_CONFIG':str(config_path),
             'MARKITAI_LOG_DIR':str(state/'logs'),'TMPDIR':str(state/'tmp')}
        with (dest/'worker.log').open('w') as log:
            done=subprocess.run([sys.executable,str(Path(__file__).resolve()),'--worker',profile,'--manifest',str(manifest_path)],
                                cwd=dest,env=env,stdout=log,stderr=log,timeout=args.timeout,check=False)
        if done.returncode: raise RuntimeError('Profile worker failed; inspect '+str(dest/'worker.log'))
    results=[]
    for case in cases:
        a=json.loads((root/'release'/(case['id']+'.json')).read_bytes())
        b=json.loads((root/'dist'/(case['id']+'.json')).read_bytes())
        na,nb=normalize(a),normalize(b); differences=differing_paths(na,nb)
        status=('equivalent_success' if a['ok'] else 'equivalent_error') if not differences else 'different'
        if differences:
            (root/(case['id']+'.diff')).write_text(''.join(difflib.unified_diff(canonical(na).splitlines(True),canonical(nb).splitlines(True),fromfile='release',tofile='dist')))
        if digest(Path(case['source']))!=case['input_sha256']: raise RuntimeError('Input changed after comparison')
        results.append({'id':case['id'],'group':case['group'],'name':case['name'],'input_sha256':case['input_sha256'],
                        'status':status,'release_ok':a['ok'],'dist_ok':b['ok'],'differences':differences,
                        'release_error':a.get('error'),'dist_error':b.get('error')})
    counts={status:sum(c['status']==status for c in results) for status in ['equivalent_success','equivalent_error','different']}
    report={'schema':1,'generated_at':dt.datetime.now(dt.timezone.utc).isoformat(),'scope':manifest['scope'],
            'source_revision':args.source_revision,'normalization':manifest['normalization'],'feature_scope':manifest['feature_scope'],
            'manifest_sha256':digest(manifest_path),'profiles':profiles,'discovery':manifest['counts'],'counts':counts,
            'worker_summaries':{p:json.loads((root/p/'summary.json').read_text()) for p in profiles},'cases':results}
    write_json(root/'report.json',report)
    print(json.dumps({'report':str(root/'report.json'),'counts':counts},indent=2))
    return int(counts['different']>0)

if __name__=='__main__':
    raise SystemExit(main())
