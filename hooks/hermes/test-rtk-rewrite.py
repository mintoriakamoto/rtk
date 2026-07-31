#!/usr/bin/env python3
"""End-to-end test harness for the installed Hermes RTK plugin.

The counterpart to `hooks/claude/test-rtk-rewrite.sh`. Where
`tests/test_rtk_rewrite_plugin.py` unit-tests the adapter with a mocked
subprocess, this harness exercises the *installed* plugin against the *real*
rtk binary — the thing the user actually runs. It is the only check that
catches a broken install: a plugin copied to the wrong directory, a stale
version, an rtk that is missing from PATH, or a rewrite contract that drifted
between the Rust binary and the Python adapter.

    python3 ~/.hermes/plugins/rtk-rewrite/../../../hooks/hermes/test-rtk-rewrite.py
    # or, from a checkout:
    python3 hooks/hermes/test-rtk-rewrite.py

Environment:
    RTK_HERMES_PLUGIN  path to the plugin's __init__.py
                       (default: ~/.hermes/plugins/rtk-rewrite/__init__.py)
    RTK_BIN            rtk binary to test against (default: rtk from PATH)

Exit code is the number of failing tests, so CI can gate on it.
"""

import importlib.util
import os
import shutil
import subprocess
import sys
from pathlib import Path

GREEN = "\033[32m"
RED = "\033[31m"
DIM = "\033[2m"
RESET = "\033[0m"

DEFAULT_PLUGIN = Path.home() / ".hermes" / "plugins" / "rtk-rewrite" / "__init__.py"
RTK_BIN = os.environ.get("RTK_BIN", "rtk")

# `rtk rewrite` exit codes: 3 = rewritten, 1 = deliberately left alone.
REWRITTEN = 3

passed = 0
failed = 0


def report(ok, description, detail=""):
    global passed, failed
    if ok:
        passed += 1
        suffix = f" {DIM}→ {detail}{RESET}" if detail else ""
        print(f"  {GREEN}PASS{RESET} {description}{suffix}")
    else:
        failed += 1
        print(f"  {RED}FAIL{RESET} {description}")
        if detail:
            print(f"       {detail}")


class HermesContext:
    """Stands in for Hermes' plugin context.

    Deliberately drives the plugin through its real `register(ctx)` entry
    point rather than reaching for the private callback, so a plugin that
    fails to register — the most likely real-world breakage — is caught.
    """

    def __init__(self):
        self.hooks = {}

    def register_hook(self, name, callback):
        self.hooks[name] = callback


def load_plugin(path):
    spec = importlib.util.spec_from_file_location("rtk_rewrite_installed", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load plugin from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rtk_rewrite(command):
    """What the engine says the command should become. The source of truth."""
    result = subprocess.run(
        ["rtk", "rewrite", command],
        capture_output=True,
        text=True,
        timeout=10,
    )
    if result.returncode == REWRITTEN:
        rewritten = result.stdout.strip()
        if rewritten and rewritten != command:
            return rewritten
    return command


def run_hook(hook, command, tool_name="terminal"):
    """Put a command through the plugin, returning what Hermes would execute.

    Hermes hands the plugin a mutable args dict and runs whatever is left in
    it, so the harness asserts on the mutation, not on a return value.
    """
    args = {"command": command}
    hook(tool_name=tool_name, args=args)
    return args["command"]


def main():
    plugin_path = Path(os.environ.get("RTK_HERMES_PLUGIN", DEFAULT_PLUGIN))

    print("=" * 44)
    print("  RTK Hermes Plugin Test Harness")
    print("=" * 44)
    print(f"  plugin: {plugin_path}")
    print(f"  rtk:    {shutil.which(RTK_BIN) or RTK_BIN}")
    print()

    if not plugin_path.is_file():
        print(f"{RED}Plugin not found at {plugin_path}{RESET}")
        print("Install it with:  rtk init --agent hermes")
        print("Or point the harness at a checkout:  RTK_HERMES_PLUGIN=hooks/hermes/rtk-rewrite/__init__.py")
        return 1

    # The plugin always invokes a bare `rtk`, so an RTK_BIN pointing at a
    # specific build only takes effect if its directory leads PATH.
    if RTK_BIN != "rtk":
        rtk_dir = str(Path(RTK_BIN).resolve().parent)
        os.environ["PATH"] = f"{rtk_dir}{os.pathsep}{os.environ.get('PATH', '')}"

    if shutil.which("rtk") is None:
        print(f"{RED}no rtk binary on PATH{RESET}")
        print("This harness tests against the real binary; install rtk first,")
        print("or point RTK_BIN at a build (its directory is prepended to PATH).")
        return 1

    plugin = load_plugin(plugin_path)

    # ---- Registration -----------------------------------------------------
    print("--- Registration ---")
    ctx = HermesContext()
    plugin.register(ctx)
    hook = ctx.hooks.get("pre_tool_call")
    report(hook is not None, "plugin registers a pre_tool_call hook")
    if hook is None:
        print("\nCannot continue without a registered hook.")
        return 1
    print()

    # ---- Contract fidelity ------------------------------------------------
    # The adapter must apply exactly what the engine decides — every rewrite
    # rule lives in Rust, so any divergence here is an adapter bug.
    print("--- Matches `rtk rewrite` exactly ---")
    commands = [
        "git status",
        "git log --oneline -10",
        "git diff HEAD",
        "gh pr list",
        "ls -la",
        "cat package.json",
        "grep -rn pattern src/",
        "rg pattern src/",
        "cargo test",
        "npm run build",
        "find . -name '*.ts'",
        "tree src/",
        "docker compose ps",
        "echo hello",
        "true",
    ]
    for command in commands:
        expected = rtk_rewrite(command)
        actual = run_hook(hook, command)
        changed = "rewritten" if expected != command else "left alone"
        report(actual == expected, f"{command}", f"{changed}: {actual}")
    print()

    # ---- Absolute guarantees ----------------------------------------------
    # Contract-fidelity alone would still pass if the plugin did nothing and
    # rtk also did nothing, so pin the behaviours that must hold outright.
    print("--- Guarantees ---")

    rewritten = run_hook(hook, "git status")
    report(
        rewritten.startswith("rtk "),
        "a supported command is actually routed through rtk",
        rewritten,
    )
    # A stray banner or diagnostic leaking into stdout would be spliced
    # straight into the command Hermes executes.
    report(
        "\n" not in rewritten and "\r" not in rewritten,
        "rewritten command is a single line (no banner contamination)",
    )

    once = run_hook(hook, "git status")
    twice = run_hook(hook, once)
    report(twice == once, "already-rewritten commands are not double-wrapped", twice)

    env_cmd = run_hook(hook, "FOO=1 git status")
    report(
        env_cmd.startswith("FOO=1 "),
        "environment prefixes are preserved",
        env_cmd,
    )

    other = run_hook(hook, "git status", tool_name="edit_file")
    report(other == "git status", "non-terminal tools are left untouched")

    blank = run_hook(hook, "   ")
    report(blank == "   ", "blank commands are left untouched")

    # Hermes must never be blocked by us: the plugin swallows its own errors
    # and leaves the command alone.
    missing_ctx = HermesContext()
    env_backup = os.environ.get("PATH", "")
    try:
        os.environ["PATH"] = ""
        plugin.register(missing_ctx)
        fallback_hook = missing_ctx.hooks.get("pre_tool_call")
        if fallback_hook is None:
            report(True, "fails open: no hook registered when rtk is absent")
        else:
            report(
                run_hook(fallback_hook, "git status") == "git status",
                "fails open: command unchanged when rtk is absent",
            )
    finally:
        os.environ["PATH"] = env_backup
    print()

    # ---- Summary ----------------------------------------------------------
    total = passed + failed
    print("=" * 44)
    if failed == 0:
        print(f"  {GREEN}ALL {total} TESTS PASSED{RESET}")
    else:
        print(f"  {RED}{failed} FAILED{RESET} / {total} total ({passed} passed)")
    print("=" * 44)
    return failed


if __name__ == "__main__":
    sys.exit(main())
