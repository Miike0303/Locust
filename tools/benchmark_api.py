"""Benchmark a Locust corpus without modifying its database or using memory.

This sends game text to the selected provider and may consume API/subscription
quota. The caller supplies an authorized corpus and provider configuration.
"""
import argparse
import json
from pathlib import Path
import sqlite3
import shutil
import hashlib
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--locust', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--provider', default='grok-sub')
    parser.add_argument('--source', default='en')
    parser.add_argument('--target', default='es')
    parser.add_argument('--rows', type=int, default=1000)
    parser.add_argument('--calibration', type=int, default=100)
    parser.add_argument('--batch-size', type=int, default=10)
    parser.add_argument('--timeout', type=float, default=3600, help='Maximum seconds per calibration or bulk run')
    args = parser.parse_args()
    if min(args.rows, args.calibration, args.batch_size, args.timeout) <= 0:
        parser.error('row counts, batch size and timeout must be positive')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    # Freeze the executable so rebuilding the workspace cannot change or lock a run.
    executable = args.output / ('locust-benchmark' + args.locust.suffix)
    shutil.copy2(args.locust.resolve(), executable)
    with executable.open('rb') as stream:
        fingerprint = hashlib.file_digest(stream, 'sha256').hexdigest()
    (args.output/'executable.sha256').write_text(fingerprint + '\n', encoding='utf-8')
    source = sqlite3.connect(args.project.resolve().as_uri()+'?mode=ro', uri=True)
    available = source.execute('select count(*) from strings').fetchone()[0]
    if available < args.rows:
        parser.error(f'corpus contains {available} entries; requested {args.rows}')
    results = []

    def run(label, concurrency, count):
        database = args.output / (label + '.db')
        db = sqlite3.connect(database)
        source.backup(db)
        db.execute('delete from strings where id not in (select id from strings order by id limit ?)', (count,))
        db.execute("update strings set translation=NULL, status='pending', provider_used=NULL, translated_at=NULL, reviewed_at=NULL")
        db.execute('delete from translation_runs')
        db.execute('delete from glossary')
        db.commit()
        db.close()
        command = [str(executable), '--config', str(args.config.resolve()),
                   'translate', str(database), '-p', args.provider, '-s', args.source,
                   '-t', args.target, '--batch-size', str(args.batch_size),
                   '--concurrency', str(concurrency), '--max-batch-tokens', '6000',
                   '--no-memory', '--context', 'Game dialogue. Translate into neutral Latin American Spanish. Preserve variables and formatting codes exactly.']
        print(f'{label}: {count} strings; concurrency={concurrency}', flush=True)
        started = time.monotonic()
        with (args.output/(label+'.log')).open('w', encoding='utf-8') as log:
            try:
                exit_code = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=args.timeout).returncode
            except subprocess.TimeoutExpired:
                exit_code = -1
        elapsed = time.monotonic()-started
        db = sqlite3.connect(database.as_uri()+'?mode=ro', uri=True)
        total, translated = db.execute('''select count(*), sum(translation is not null and translation != '') from strings''').fetchone()
        usage = db.execute('select sum(tokens_used),sum(input_tokens),sum(output_tokens),sum(cost_usd),min(cost_is_complete) from translation_runs').fetchone()
        db.close()
        result = dict(label=label, concurrency=concurrency, batch_size=args.batch_size,
                      exit_code=exit_code, strings=total, translated=translated or 0,
                      seconds=round(elapsed, 3), strings_per_minute=round((translated or 0)/elapsed*60, 2),
                      tokens=usage[0], input_tokens=usage[1], output_tokens=usage[2],
                      observed_cost=usage[3], cost_is_complete=exit_code == 0 and bool(usage[4]))
        results.append(result)
        (args.output/'results.json').write_text(json.dumps(results, indent=2), encoding='utf-8')
        print(json.dumps(result), flush=True)
        return result

    for concurrency in [1, 3, 6, 10]:
        run(f'calibration-{concurrency}', concurrency, min(args.calibration, args.rows))
    good = [r for r in results if r['exit_code'] == 0 and r['translated'] == r['strings']]
    if not good:
        raise SystemExit('No complete calibration succeeded; bulk test was not dispatched.')
    best = max(good, key=lambda result: result['strings_per_minute'])
    run('bulk', best['concurrency'], args.rows)
    source.close()


if __name__ == '__main__':
    main()
