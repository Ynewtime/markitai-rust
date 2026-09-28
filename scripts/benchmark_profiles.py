#!/usr/bin/env python3
"""Measure two frozen native profiles with alternating order and exact outputs."""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project-root', type=Path, default=Path.cwd())
    parser.add_argument('--reference', type=Path, required=True)
    parser.add_argument('--release-artifacts', type=Path, required=True)
    parser.add_argument('--dist-artifacts', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--python', type=Path, default=Path(sys.executable))
    parser.add_argument('--cli-iterations', type=int, default=5)
    parser.add_argument('--api-processes', type=int, default=3)
    parser.add_argument('--api-iterations', type=int, default=20)
    parser.add_argument('--timeout', type=int, default=300)
    args = parser.parse_args()
    if min(args.cli_iterations, args.api_processes, args.api_iterations) < 3:
        parser.error('Use at least three CLI iterations, API processes and API calls')
    if args.timeout <= 0:
        parser.error('Worker timeout must be positive')
    if args.output.exists():
        parser.error('Use a fresh output directory; earlier evidence is never overwritten')
    root, reference, output = (p.resolve() for p in (args.project_root, args.reference, args.output))
    if output == reference or reference in output.parents:
        parser.error('Output must be outside the read-only reference checkout')
    sys.path.insert(0, str(root / 'scripts'))
    import benchmark_api as api

    profiles = {name: json.loads(path.read_text()) for name, path in (
        ('release', args.release_artifacts), ('dist', args.dist_artifacts))}
    if profiles['release']['source_revision'] != profiles['dist']['source_revision']:
        raise RuntimeError('Profile source revisions differ')
    if profiles['release']['code_trees'] != profiles['dist']['code_trees']:
        raise RuntimeError('Compiled source/manifest trees differ')
    for name, record in profiles.items():
        if record['profile'] != name or record['source_status']:
            raise RuntimeError('Profile identity is wrong or its source was dirty')
        for item in record['artifacts'].values():
            if digest(item['path']) != item['sha256']:
                raise RuntimeError('A frozen artifact changed')
    output.mkdir(parents=True)
    synthetic = output / 'fixtures'
    synthetic.mkdir()
    (synthetic / 'input.txt').write_text('# Benchmark\n\n' + 'A paragraph with facts and 中文.\n' * 3000)
    (synthetic / 'input.csv').write_text('name,value\n' + ''.join(f'row{i},{i}\n' for i in range(1000)))
    fixtures = reference / 'packages/markitai/tests/fixtures'
    cases = [
        ('text_105k', synthetic / 'input.txt'),
        ('csv_1000_rows', synthetic / 'input.csv'),
        ('wikipedia_html', reference / 'packages/markitai/tests/defuddle_fixtures/fixtures/general--wikipedia.html'),
        ('sample_pdf', fixtures / 'sample.pdf'),
        ('sample_docx', fixtures / 'sample.docx'),
        ('sample_pptx', fixtures / 'sample.pptx'),
    ]
    input_hashes = {name: digest(path) for name, path in cases}
    ref_status = subprocess.check_output(['git', 'status', '--porcelain'], cwd=reference, text=True)
    report = {
        'schema': 1, 'created_at': dt.datetime.now(dt.timezone.utc).isoformat(),
        'platform': platform.platform(), 'python': sys.version,
        'script_sha256': digest(__file__), 'api_worker_sha256': digest(api.__file__),
        'profiles': profiles,
        'reference_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=reference, text=True).strip(),
        'reference_status_before': ref_status, 'completed': False,
        'cli_iterations': args.cli_iterations, 'api_processes': args.api_processes,
        'api_iterations': args.api_iterations,
        'cli_warmup_processes_per_profile_per_case': 1, 'api_warmups_per_worker': 1,
        'measurement': {
            'cli': 'fresh native process, pure stdout, buffered output, warm filesystem; alternating profile order per iteration',
            'api': 'pre-encoded C ABI through Python ctypes; existing benchmark_api worker includes decode/free; alternating profile order per fresh worker pair',
            'api_rss': 'worker lifetime peak including Python host, imports, native library and conversions',
            'asset_output': 'no output directory; in-memory conversion, without persisting converted assets',
        },
        'limitations': [
            'Only six fixed inputs; no claim of all-document or cold-cache performance.',
            'No complete Python/Node/Go public-wrapper timing; no long-term leak test.',
            'Both profiles share known extraction differences from the Python reference.',
            'This run compares native profiles only, not the reference implementation.',
            'Configuration isolation and Python audit hooks are not an OS sandbox for native system calls.',
            'CPU scheduling and unrelated system work are uncontrolled; small differences may be noise.',
        ], 'cases': [],
    }
    write(output / 'report.json', report)
    for index, (name, source) in enumerate(cases):
        case_dir = output / name
        case_dir.mkdir()
        state = case_dir / 'cli-state'
        state.mkdir()
        cfg = api.configuration(state)
        cfg['log'] = {'dir': None}
        cfg['fetch'] = {'no_remote': True, 'remote_consent': 'never'}
        cfg['image']['stdout_fetch_external'] = False
        config_path = case_dir / 'config.json'
        write(config_path, cfg)
        env = {key: os.environ[key] for key in ('PATH', 'SYSTEMROOT', 'LANG', 'LC_ALL', 'TMPDIR') if key in os.environ}
        env.update(MARKITAI_HOME=str(state), MARKITAI_CONFIG=str(config_path),
                   PYTHONNOUSERSITE='1', PYTHON_DOTENV_DISABLED='1', PYTHONDONTWRITEBYTECODE='1')
        commands = {p: [r['artifacts']['cli']['path'], str(source), '--pure', '--no-llm',
                       '--no-ocr', '--no-alt', '--no-desc', '--no-screenshot'] for p, r in profiles.items()}
        expected, samples = {}, {p: [] for p in profiles}
        order = list(profiles) if index % 2 == 0 else list(reversed(profiles))

        def invoke(label):
            start = time.perf_counter_ns()
            result = subprocess.run(commands[label], cwd=case_dir, env=env,
                                    capture_output=True, timeout=args.timeout, check=False)
            elapsed = (time.perf_counter_ns() - start) / 1_000_000
            if result.returncode:
                (case_dir / f'{label}-failure.stderr').write_bytes(result.stderr)
                raise RuntimeError(f'{name}/{label} exited {result.returncode}')
            return elapsed, result.stdout, result.stderr

        for label in order:
            _, stdout, stderr = invoke(label)
            expected[label] = (stdout, stderr)
            (case_dir / f'{label}-warmup.stdout').write_bytes(stdout)
            (case_dir / f'{label}-warmup.stderr').write_bytes(stderr)
        if expected['release'] != expected['dist']:
            raise RuntimeError(f'{name}: CLI stdout or stderr differs between profiles')
        for iteration in range(args.cli_iterations):
            for label in order if iteration % 2 == 0 else list(reversed(order)):
                elapsed, stdout, stderr = invoke(label)
                if (stdout, stderr) != expected[label]:
                    raise RuntimeError(f'{name}/{label}: repeated CLI output changed')
                samples[label].append(elapsed)
        cli_results = {p: {
            'samples_ms': values, 'median_ms': statistics.median(values),
            'stdout_sha256': hashlib.sha256(expected[p][0]).hexdigest(),
            'stdout_bytes': len(expected[p][0]),
            'stderr_sha256': hashlib.sha256(expected[p][1]).hexdigest(),
        } for p, values in samples.items()}
        api_results = {p: [] for p in profiles}
        api_expected = None
        for process_index in range(args.api_processes):
            for label in order if process_index % 2 == 0 else list(reversed(order)):
                directory = case_dir / f'api-{process_index}-{label}'
                worker = api.run_worker(args.python.absolute(), {
                    'engine': 'native', 'source': str(source),
                    'library': profiles[label]['artifacts']['ffi']['path'],
                    'iterations': args.api_iterations,
                }, directory, args.timeout)
                markdown = (directory / 'output.md').read_bytes()
                observable = (markdown, worker['warnings'])
                if api_expected is None:
                    api_expected = observable
                elif observable != api_expected:
                    raise RuntimeError(f'{name}/{label}: C-ABI profile or worker output differs')
                api_results[label].append(worker)
        api_summary = {p: {
            'median_of_process_medians_ms': statistics.median(w['median_ms'] for w in workers),
            'median_peak_rss_bytes': statistics.median(w['peak_rss_bytes'] for w in workers),
            'workers': workers,
        } for p, workers in api_results.items()}
        if digest(source) != input_hashes[name]:
            raise RuntimeError(f'{name}: source input changed')
        report['cases'].append({
            'name': name, 'source': str(source), 'input_bytes': source.stat().st_size,
            'input_sha256': input_hashes[name], 'cli_exact_stdout_and_stderr': True,
            'api_exact_markdown_and_warnings': True, 'cli': cli_results, 'api': api_summary,
        })
        write(output / 'report.json', report)
        print(f'{name}: exact CLI/C-ABI output; measurements saved', flush=True)
    for record in profiles.values():
        for item in record['artifacts'].values():
            if digest(item['path']) != item['sha256']:
                raise RuntimeError('Frozen artifact changed during measurement')
    report['reference_status_after'] = subprocess.check_output(['git', 'status', '--porcelain'], cwd=reference, text=True)
    if report['reference_status_after'] != ref_status:
        raise RuntimeError('Reference checkout status changed')
    report['completed'] = True
    report['finished_at'] = dt.datetime.now(dt.timezone.utc).isoformat()
    write(output / 'report.json', report)
    print(output / 'report.json', flush=True)


if __name__ == '__main__':
    main()
