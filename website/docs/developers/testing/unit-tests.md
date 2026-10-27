---
sidebar_position: 1
title: Unit Tests
description: Basic unit tests for individual components
slug: /developers/testing/unit-tests
---

# Unit Tests

## Overview

Basic unit tests for individual components provide the foundation of the testing
infrastructure. These tests focus on testing individual functions, modules, and
components in isolation.

## Running Unit Tests

### Standard Unit Tests

Run all unit tests across the codebase:

```bash
# Run all unit tests
make test

# Run tests in release mode
make test-release

# Use cargo-nextest for faster execution
make nextest
```

### Public-network CLI bootstrap

`cargo test --release -p mina-cli` skips the public devnet bootstrap test.
Running ignored tests alone does not opt in to public-network access. To run
that test explicitly:

```bash
MINA_TEST_PUBLIC_NETWORK=1 cargo test --release -p mina-cli --test bootstrap -- --ignored --exact bootstrap --nocapture
```

The test starts a devnet follower and uses a temporary node workdir, overriding
`MINA_HOME`. The child receives only the test environment and the inherited
`PATH`, `BPF_ALIAS`, `NO_PEER_DISCOVERY`, and `MINA_RECORD` settings. It removes
that workdir after stopping and reaping the child, including on timeout. HTTP
port 49998 and libp2p port 49999 must be free. `MINA_COMMAND` can select an
alternate node executable. Set `OUT_PATH` to a log file prefix to keep stdout
and stderr after the temporary workdir is removed. The dedicated CI bootstrap
job supplies both opt-ins.

The P2P test that resolves a nonexistent domain also requires explicit public
DNS access:

```bash
MINA_TEST_PUBLIC_NETWORK=1 cargo test --release -p mina-p2p --lib -- --ignored --exact webrtc::host::tests::test_resolve_invalid_domain
```

On a shared development machine, obtain a node slot and approval before running
this live test, following the repository's lane rules.

### Performance Considerations

1. **Resource management**: Monitor memory and CPU usage during tests
2. **Timeout handling**: Set appropriate timeouts for test conditions
3. **Parallel execution**: Run independent tests concurrently
4. **Cleanup**: Ensure proper resource cleanup after tests

## Best Practices

### Test Design

1. **Isolation**: Ensure tests don't depend on external state
2. **Deterministic**: Tests should produce consistent results
3. **Fast**: Unit tests should execute quickly
4. **Clear**: Test names should clearly describe what is being tested

### Debugging Unit Tests

1. **Enable logging**: Use detailed logging for test debugging
2. **Check assumptions**: Ensure test assumptions match actual behavior
3. **Reproduce locally**: Run failing tests locally for easier debugging
4. **Update expectations**: Adjust test expectations if behavior changed

## Integration with CI

Unit tests run automatically in CI with:

- **Container isolation**: Each test runs in clean container
- **Parallel execution**: Multiple tests run concurrently
- **Artifact collection**: Test results archived for analysis

## Related Documentation

- [Testing Framework Overview](testing-framework): Main testing documentation
- [Scenario Tests](scenario-tests): Integration testing scenarios
- [Getting Started](../getting-started): Basic development setup including
  testing
