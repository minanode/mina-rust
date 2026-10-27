# Status response fixtures

Recorded by GET from relay2, PID 1090419, HTTP 3300, on 2026-10-21 UTC:

- `relay2-status.json`: `/status`, around 04:23; tip 564635, six connected
  libp2p peers, timestamp 1792556460000000000 ns advertised for that exact tip.
- `relay2-build.json`: `/build_env`, 04:24:03; remote commit `85225b2e`.
- `relay2-actions.json`: `/stats/actions?id=latest`, 04:24:03; latest action
  window at height 564635, all four verification counters zero.

The status fixture retains chain, frontier, resources and peer fields used by
this command; peer addresses/IDs and unrelated fields were omitted. The actions
fixture retains the four verification action kinds and their original latency
buckets. No retained values were changed. Expected parser values come from these
responses, not from the CLI's output. Missing-field, transport and clock cases
deliberately mutate copies to test failure behavior.
