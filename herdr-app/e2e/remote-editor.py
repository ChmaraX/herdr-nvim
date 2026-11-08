"""Live remote editor test. Creates and removes its own remote space and /tmp files."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time


def execute(argv, *, env=None, check=True, timeout=35):
    result = subprocess.run(argv, text=True, capture_output=True, env=env, timeout=timeout)
    if check and result.returncode:
        raise RuntimeError(result.stderr or result.stdout)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--machine", required=True)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--home", default=os.environ.get("HERDR_APP_HOME", str(Path.home() / ".config/herdr-app")))
    args = parser.parse_args()
    env = {**os.environ, "HERDR_NVIM_HOST": "app", "HERDR_APP_HOME": args.home}

    def app(command, params):
        return json.loads(execute(["herdr-app", "do", command, json.dumps(params)], env=env).stdout)

    def remote(argv):
        return execute(["hc", "exec", "--on", args.machine, "--timeout", "15", "--", *argv]).stdout

    folder = remote(["mktemp", "-d", "/tmp/herdr-nvim-remote-test-XXXXXX"]).strip()
    file = folder + "/notes 'unicode-雪'.md"
    remote(["sh", "-c", 'printf "%s\\n" "# Remote editor test" > "$1"', "test", file])
    created = None
    try:
        created = app("space.new", {"name": "Remote editor test", "folder": folder, "machine": args.machine})
        ctx = {"space": created["space"], "tab": created["tab"], "terminal": created["terminal"], "cwd": folder}
        time.sleep(0.5)

        def call(mode, *rest, check=True):
            return execute([args.binary, mode, "--ctx", json.dumps(ctx), *rest], env=env, check=check)

        space = ctx["space"].split("/", 1)[1]
        tab = ctx["tab"].split("/", 1)[1]
        key = hashlib.sha256(f"{space}:{tab}".encode()).hexdigest()[:20]
        prefix = remote(["nvim", "--clean", "--headless", "-c", "lua io.stdout:write(vim.uv.os_tmpdir()..'/herdr-nvim-remote-'..vim.uv.getuid())", "-c", "qa"]).strip()
        socket = prefix + "/" + key + ".sock"

        def evaluate(lua):
            expr = "luaeval(" + json.dumps(lua) + ")"
            return json.loads(remote(["nvim", "--clean", "--headless", "--server", socket, "--remote-expr", expr]).strip())

        opened = json.loads(call("open", file + ":1:1").stdout)["terminal"]
        assert opened.startswith(args.machine + "/")
        evaluate('(function() vim.api.nvim_buf_set_lines(0,0,-1,false,{"# Remote editor test", "", "Unsaved buffer preserved"}); vim.api.nvim_win_set_cursor(0,{3,0}); return vim.json.encode(true) end)()')
        refused = call("app-remote-stop", check=False)
        assert refused.returncode and "unsaved" in refused.stderr
        print("PASS: remote Stop refuses unsaved work", flush=True)

        closed = json.loads(call("toggle", "--state", "close").stdout)
        assert closed["closed"] == opened
        state = evaluate("vim.json.encode({modified=vim.bo.modified,cursor=vim.api.nvim_win_get_cursor(0)})")
        assert state == {"modified": True, "cursor": [3, 0]}
        reopened = json.loads(call("toggle", "--state", "open").stdout)["terminal"]
        assert reopened == opened
        print("PASS: hide/reopen preserves the daemon, buffer, cursor and UI address", flush=True)

        evaluate('(function() vim.cmd.write(); return vim.json.encode(true) end)()')
        contents = execute(["hc", "file", "read", "--on", args.machine, file]).stdout
        assert "Unsaved buffer preserved" in contents
        assert not Path(file).exists()
        print("PASS: writes affect only the remote file", flush=True)

        call("app-remote-stop")
        call("open", file)
        app("space.close", {"id": created["space"], "confirmed": True})
        created = None
        deadline = time.monotonic() + 15
        while True:
            exists = remote(["sh", "-c", 'test -S "$1" && printf yes || printf no', "test", socket]).strip()
            if exists == "no":
                break
            assert time.monotonic() < deadline, "Closing the space left Neovim running"
            time.sleep(1)
        print("PASS: closing the space stops its remote daemon", flush=True)
    finally:
        if created:
            app("space.close", {"id": created["space"], "confirmed": True})
        remote(["rm", "-rf", "--", folder])


if __name__ == "__main__":
    main()
