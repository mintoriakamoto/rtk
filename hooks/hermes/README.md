# RTK Plugin for Hermes

Rewrites Hermes `terminal` tool commands to RTK equivalents before execution, so Hermes receives compact command output without changing your workflow.

## Installation

```bash
rtk init --agent hermes
```

The installer writes the plugin to `~/.hermes/plugins/rtk-rewrite/` and enables it through `plugins.enabled` in the Hermes config. The repository copy lives in `hooks/hermes/`; don't use that repo path as the runtime install path.

## Development

Run the Hermes plugin unit tests from the repository root. These mock the rtk
subprocess, so they check the adapter's logic in isolation:

```bash
python3 -m unittest discover -s hooks/hermes
```

## Verifying a real install

`test-rtk-rewrite.py` is the end-to-end harness — the Hermes counterpart to
`hooks/claude/test-rtk-rewrite.sh`. It drives the **installed** plugin through
its real `register(ctx)` entry point against the **real** rtk binary, so it
catches what the mocked unit tests cannot: a plugin installed to the wrong
directory, a stale copy, an rtk missing from PATH, or an adapter that has
drifted from the engine's rewrite contract.

```bash
python3 hooks/hermes/test-rtk-rewrite.py
```

By default it tests `~/.hermes/plugins/rtk-rewrite/__init__.py`. Override with
`RTK_HERMES_PLUGIN` to test a checkout, and `RTK_BIN` to test a specific build
(its directory is prepended to `PATH`, since the plugin always invokes a bare
`rtk`):

```bash
RTK_HERMES_PLUGIN=hooks/hermes/rtk-rewrite/__init__.py \
RTK_BIN=target/release/rtk \
  python3 hooks/hermes/test-rtk-rewrite.py
```

It exits with the number of failing tests, so CI can gate on it.

Most assertions compare the plugin's result against `rtk rewrite` directly
rather than hardcoding expected strings: every rewrite rule lives in Rust, so
the adapter's job is to apply the engine's decision faithfully, and pinning
literals here would only rot when rtk's rules change. A handful of absolute
guarantees are pinned outright — a supported command really is routed through
rtk, commands are never double-wrapped, env prefixes survive, non-terminal
tools and blank commands are untouched, and a missing rtk fails open.

## How it works

Hermes loads plugins from Python, so the plugin entrypoint is Python. The Python code is only a thin Hermes adapter. It reads the Hermes terminal tool payload, calls `rtk rewrite` for the actual command decision, then mutates the terminal tool `command` before Hermes executes it.

All rewrite rules stay in Rust inside `rtk rewrite`. When RTK adds or changes command rewrite behavior, the Hermes plugin picks up that behavior by delegating to the RTK binary.

## Fail-open behavior

The plugin does not block command execution. If anything goes wrong, Hermes runs the original command unchanged.

If rtk is not available in PATH when Hermes loads the plugin, the plugin prints a warning and skips hook registration.

- `rtk` is missing from `PATH`
- `rtk rewrite` exits with an error
- Hermes sends a non-terminal tool call
- The tool payload has no string `command`
- The plugin raises an unexpected exception

## Limitations

- Only Hermes `terminal` tool calls are rewritten.
- Commands skipped by `rtk rewrite` stay unchanged, including commands already prefixed with `rtk`, compound shell commands, heredocs, and commands without an RTK filter.
- Shell hooks are not used for Hermes command rewriting. The integration depends on Hermes loading Python plugins and passing a mutable terminal tool payload.
