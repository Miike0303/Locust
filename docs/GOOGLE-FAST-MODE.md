# Google adaptive fast mode

Measured against `https://translate.googleapis.com/translate_a/t?client=gtx`
on 2026-10-28. The conservative 8 RPM setting was limiting **engine batches**,
not HTTP requests: a default 200-string batch could issue four 50-source POSTs.
The new budget counts every actual POST, including retries and split fallbacks.

## Defaults and evidence

| Setting | Default |
| --- | --- |
| Initial HTTP rate | 60 requests/minute |
| Additive increase | +2 RPM per validated successful response |
| Maximum HTTP rate | 140 RPM |
| Multiplicative decrease | halve on 429, 503, or a traffic/captcha challenge |
| Adaptive floor | 8 RPM |
| Shared cooldown | 30 seconds; consecutive blocks extend this to 60/120/240 seconds |
| Chunk limits | 200 sources, 9,000 Unicode characters, 64 KiB encoded form body |
| Engine concurrency | 4 (existing shared engine/CLI default retained) |
| Internal chunk concurrency | 4 |

The 200-source candidate translated 2,000 English fixture strings in 8.217 s
(14,604.42 strings/minute), versus 60.130 s (1,995.68 strings/minute) at the old
settings: approximately 7.3 times faster. Increasing concurrency from 4 to 8 or
16 did not materially improve the paced runs. Larger chunks reduced the number
of HTTP requests from 40 to 20 to 10 while preserving one translation per source.
POST form bodies were already used; source text never enters the URL query.

The 140 RPM ceiling is the highest **final rate reached** by the successful
40-request probes. It is not a measured sustained service quota. The probes do
not establish a universal IP-block threshold; this ceiling avoids an unmeasured
240+ RPM ramp. The old cap was not supported by these results.

## Limiter and retry behavior

All normal Google provider instances share one process-wide limiter. A fair
Tokio mutex queues HTTP admission; a separate short state lock protects rate,
cooldown and counters. Requests are paced rather than released in minute-sized
bursts. Feedback wakes the sleeping queue head, which recomputes its deadline.
Successes already in flight during cooldown cannot increase the rate or shorten
the cooldown. The limiter does not coordinate separate Locust processes.

`ProviderConfig.extra["requests_per_minute"]` remains the override. A positive
integer selects a fixed **HTTP** rate and disables AIMD. Fixed rates still honor
traffic cooldowns and `Retry-After`. Invalid/zero overrides retain the defaults.
Both delta-seconds and standard HTTP-date `Retry-After` values are accepted; the
wait is at least the server's requested duration and the local cooldown.

Core's existing outer `acquire()` performs readiness-only admission for Google,
so initial queueing remains outside the retry deadline without double-charging
an HTTP request. The provider's `acquire_http()` consumes the allowance before
every POST. Retry waits and all subrequests remain inside the existing overall
deadline. Existing biased cancellation still wins; dropping a provider future
aborts its JoinSet and releases queued positions without reserving future slots.

429/503 feedback is processed from response headers without waiting for a body.
HTML/captcha/unusual-traffic responses are rate-limit errors and bypass splitting.
Challenge bodies are never echoed in errors. Valid JSON dialogue mentioning
"unusual traffic" is accepted. Other malformed responses retain the existing
split-in-half fallback, input order, and per-source validation.

The body budget counts percent-encoded UTF-8, including repeated `q` fields.
A source above the character limit travels alone, preserving its text. A single
source above the encoded body budget fails explicitly without sending/truncating
it. The body ceiling is a local bound for this undocumented endpoint, not a
claim about an official Google API limit.

## Reproducing the bounded probe

```powershell
cargo test -p locust-providers --test google_fast_probe -- --ignored --nocapture
```

The ignored integration test uses exactly 2,000 indexed synthetic strings per
language pair, based on `tests/integration/main.rs` and the existing Google live
smoke fixture. It reads no game files. Every configuration uses the same corpus
and 200-string engine batches. Configuration concurrency is engine concurrency;
there are only ten engine batches, so the concurrency-16 case can occupy at most
ten. Internal concurrency is 8 for the legacy baseline and 4 for adaptive runs.

Each configuration is capped at 180 seconds, and the whole invocation at 900
seconds. Three consecutive 429/blocked responses latch a circuit and immediately
wake the supervisor, which aborts all tasks even during retry/backoff. After any
configuration that observed a block, the suite also stops rather than discarding
cooldown state between configurations. Three wholly unsuccessful transport-error
configurations also stop the suite, with five-second pauses between them.

`LOCUST_GOOGLE_PROBE_BUDGET_SECONDS` can reduce (never increase) the 900-second
budget. `LOCUST_GOOGLE_PROBE_SKIP` skips completed configurations in their printed
order, allowing a bounded interrupted run to resume. When resuming, account for
time already spent probing and preserve any active server cooldown.

Throughput is completed/validated strings divided by wall time, including rate
queueing and retries. The CSV also records POST count, blocks, first-429 time,
first-block time, final rate, and completion status. A transport failure is not
counted as an HTTP 429.

## Raw measurements

The CSV below contains the completed comparison runs and the earlier failed
Japanese baseline. Preliminary measurements and the session budget are recorded
after the comparison data.

In legacy rows, `final_rpm=4294967295` denotes the unlimited internal HTTP
instrumentation; the separate original 8-batch/minute gate is still active.

```csv
config,pair,chunk_sources,chunk_chars,concurrency,strings,seconds,strings_per_min,http_requests,http_429,blocked,first_429_seconds,first_block_seconds,final_rpm,outcome
legacy,en->es,50,4500,4,2000,60.130,1995.68,40,0,0,none,none,4294967295,complete
adaptive,en->es,50,4500,4,2000,27.021,4440.92,40,0,0,none,none,140,complete
adaptive,en->es,50,4500,8,2000,28.003,4285.25,40,0,0,none,none,140,complete
adaptive,en->es,50,4500,16,2000,26.329,4557.74,40,0,0,none,none,140,complete
adaptive,en->es,100,9000,4,2000,15.642,7671.60,20,0,0,none,none,100,complete
adaptive,en->es,100,9000,8,2000,15.215,7886.94,20,0,0,none,none,100,complete
adaptive,en->es,100,9000,16,2000,15.393,7795.63,20,0,0,none,none,100,complete
adaptive,en->es,200,9000,4,2000,8.217,14604.42,10,0,0,none,none,80,complete
legacy,ja->en,50,4500,4,2000,62.038,1934.30,40,0,0,none,none,4294967295,complete
adaptive,ja->en,50,4500,4,2000,26.946,4453.38,40,0,0,none,none,140,complete
adaptive,ja->en,50,4500,8,2000,25.965,4621.54,40,0,0,none,none,140,complete
adaptive,ja->en,50,4500,16,2000,26.109,4596.14,40,0,0,none,none,140,complete
adaptive,ja->en,100,9000,4,2000,15.563,7710.50,20,0,0,none,none,100,complete
adaptive,ja->en,100,9000,8,2000,15.530,7727.18,20,0,0,none,none,100,complete
adaptive,ja->en,100,9000,16,2000,15.227,7880.53,20,0,0,none,none,100,complete
adaptive,ja->en,200,9000,4,2000,8.227,14585.96,10,0,0,none,none,80,complete
```

Earlier Japanese baseline attempt (transport error; recovered on the bounded retry):

```csv
config,pair,chunk_sources,chunk_chars,concurrency,strings,seconds,strings_per_min,http_requests,http_429,blocked,first_429_seconds,first_block_seconds,final_rpm,outcome
legacy,ja->en,50,4500,4,0,33.486,0.00,16,0,0,none,none,4294967295,error: provider error: Google Translate request failed: error sending request for url (https://translate.googleapis.com/translate_a/t?client=gtx&sl=ja&tl=en&dj=1)
```

Preliminary completed EN→ES runs before the supervisor safety improvements; no blocked responses occurred:

```csv
config,pair,chunk_sources,chunk_chars,concurrency,strings,seconds,strings_per_min,http_requests,http_429,blocked,first_429_seconds,first_block_seconds,final_rpm,outcome
legacy,en->es,50,4500,4,2000,62.368,1924.07,40,0,0,none,none,4294967295,complete
adaptive,en->es,50,4500,4,2000,25.849,4642.38,40,0,0,none,none,140,complete
adaptive,en->es,50,4500,8,2000,25.902,4632.86,40,0,0,none,none,140,complete
```

The recorded configurations, including the failed baseline and preliminary runs, total 539.160 seconds (about 9.0 minutes). The preliminary invocation was interrupted to improve probe supervision; the two subsequent invocations were bounded to 600 and 420 seconds respectively. With the small connectivity checks and interrupted work, total live probing remained below 15 minutes. No configuration approached its 180-second limit.

All successful comparison rows translated all 2,000 strings. Every row recorded zero HTTP 429s, zero traffic challenges/503 blocks, and no first-429/first-block time. The transport failure is retained rather than treated as evidence of a block.

