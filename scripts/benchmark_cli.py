#!/usr/bin/env python3
"""Measure end-to-end CLI startup/conversion, with an explicit reference."""
from __future__ import annotations
import argparse
import datetime
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time


def run(command, environment, cwd):
    start = time.perf_counter_ns()
    result = subprocess.run(command, env=environment, cwd=cwd, capture_output=True, timeout=60)
    elapsed = (time.perf_counter_ns() - start) / 1e6
    if result.returncode:
        raise RuntimeError(f"Command failed ({result.returncode}): {command[0]}: {result.stderr.decode(errors='replace')[:500]}")
    return elapsed, result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/release/markitai'))
    parser.add_argument('--reference', type=Path, required=True)
    parser.add_argument('--iterations', type=int, default=7)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.iterations < 3:
        parser.error('At least 3 measured iterations are required')
    binary, reference = args.binary.resolve(), args.reference.resolve()
    old = reference / '.venv/bin/python'
    if not binary.is_file() or not old.is_file():
        parser.error('Build the Rust CLI and provide an installed reference .venv')
    report = {'created_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'platform':platform.platform(), 'machine':platform.machine(),
              'iterations':args.iterations, 'warmup':1,
              'measurement':'fresh CLI process, buffered stdout; OS filesystem cache warm',
              'reference_sha':subprocess.check_output(['git','rev-parse','HEAD'],cwd=reference,text=True).strip(),
              'binary':{'bytes':binary.stat().st_size,
                        'gzip_bytes':len(gzip.compress(binary.read_bytes(),mtime=0)),
                        'sha256':hashlib.sha256(binary.read_bytes()).hexdigest()}, 'cases':[]}
    with tempfile.TemporaryDirectory(prefix='markitai-bench-') as work:
        work = Path(work)
        config = work/'config.json'
        config.write_text(json.dumps({'llm':{'enabled':False},'ocr':{'enabled':False},
            'screenshot':{'enabled':False},'image':{'alt_enabled':False,'desc_enabled':False},
            'cache':{'enabled':False,'global_dir':str(work/'cache')},'history':{'record':False}}))
        environment = {k:v for k,v in os.environ.items() if k in {'PATH','SYSTEMROOT','TMPDIR','LANG','LC_ALL'}}
        environment.update(MARKITAI_HOME=str(work/'state'),MARKITAI_CONFIG=str(config),
                           MARKITAI_LOG_DIR=str(work/'logs'),PYTHONDONTWRITEBYTECODE='1',
                           LITELLM_LOCAL_MODEL_COST_MAP='True')
        cases = {
            'startup_version':None,
            'text_100k':('input.txt', '# Benchmark\n\n'+('A paragraph with facts and 中文.\n'*3000)),
            'csv_1000_rows':('input.csv','name,value\n'+''.join(f'row{i},{i}\n' for i in range(1000))),
            'html_article':('input.html','<html><title>Benchmark</title><nav>Navigation</nav><article><h1>Benchmark</h1>'+('<p>A paragraph with <strong>facts</strong> and <a href="https://example.com">a link</a>.</p>'*100)+'</article></html>'),
        }
        for name, fixture in cases.items():
            args_tail = ['--version']
            if fixture:
                path = work/fixture[0]
                path.write_text(fixture[1])
                args_tail = [str(path),'--pure','--no-llm','--no-ocr','--no-alt','--no-desc','--no-screenshot']
            commands = {'rust':[str(binary),*args_tail], 'python':[str(old),'-m','markitai',*args_tail]}
            measurements, payloads = {}, {}
            for label,command in commands.items():
                run(command,environment,work)
                samples=[]
                for _ in range(args.iterations):
                    elapsed,payload=run(command,environment,work)
                    samples.append(elapsed)
                payloads[label]=payload
                measurements[label]={'median_ms':round(statistics.median(samples),3),
                                     'min_ms':round(min(samples),3),'max_ms':round(max(samples),3),
                                     'samples_ms':[round(n,3) for n in samples],
                                     'stdout_bytes':len(payload)}
            report['cases'].append({'name':name, 'measurements':measurements,
                'stdout_exact_match':payloads['rust']==payloads['python'] if fixture else None,
                'median_ratio_python_over_rust':round(measurements['python']['median_ms']/measurements['rust']['median_ms'],2)})
            print(f"{name}: Rust {measurements['rust']['median_ms']} ms; Python {measurements['python']['median_ms']} ms",flush=True)
    report['limitations']=['Small synthetic warm-filesystem cases; not broad document throughput.',
        'Each measurement includes process startup, parsing, and buffered stdout.',
        'Nonmatching output cases are not equivalent-quality speedup evidence.',
        'No installed Python package-size comparison: reference .venv contains development dependencies.',
        'No peak RSS or cold filesystem measurement in this initial run.']
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')

if __name__=='__main__':
    main()
