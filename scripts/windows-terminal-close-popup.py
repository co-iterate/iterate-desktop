"""Observe terminal-close lifecycle through real MCP stdio and Windows UI.

Uses installed binaries and saved request content. Does not launch an AI model.
Two dedicated Windows console windows share a test-only WebView data folder.
Artifacts contain local request data; keep the evidence directory untracked.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.request
import uuid

import psutil
from PIL import ImageGrab
from pywinauto import Desktop


def write_json(path, data):
    Path(path).write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")


def wait_for(probe, timeout=50):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = probe()
        if result:
            return result
        time.sleep(0.25)
    raise TimeoutError("Observable state not reached")


def host(config_path):
    config = json.loads(Path(config_path).read_text(encoding="utf-8"))
    os.environ.update(config["env"])
    os.chdir(config["workspace"])
    ctypes.windll.kernel32.SetConsoleTitleW(config["title"])
    stderr = open(config["stderr"], "w", encoding="utf-8")
    process = subprocess.Popen([config["mcp"], str(config["port"])],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=stderr, text=True, encoding="utf-8")
    write_json(config["state"], {"host_pid": os.getpid(), "mcp_pid": process.pid})

    def send(message):
        process.stdin.write(json.dumps({"jsonrpc": "2.0", **message}) + "\n")
        process.stdin.flush()

    def receive(target):
        for line in process.stdout:
            message = json.loads(line)
            if message.get("id") == target:
                return message
        raise RuntimeError("MCP stdout closed")

    send({"id": 1, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "iterate-terminal-close-test", "version": "1"}}})
    write_json(config["initialize"], receive(1))
    send({"method": "notifications/initialized"})
    send({"id": 2, "method": "tools/call", "params": {
        "name": "call_zhi", "arguments": config["arguments"]}})
    print("Real MCP popup pending. This is a dedicated lifecycle test terminal.", flush=True)
    write_json(config["response"], receive(2))
    print("MCP response received. Keeping test terminal open.", flush=True)
    time.sleep(240)


def descendants(pid):
    try:
        return psutil.Process(pid).children(recursive=True)
    except psutil.NoSuchProcess:
        return []


def terminal(title):
    return next((Desktop(backend="uia").window(handle=w.handle).wrapper_object()
                 for w in Desktop(backend="win32").windows()
                 if title in w.window_text() and w.is_visible()), None)


def popup(pid):
    child_pids = {p.pid for p in descendants(pid) if p.name().lower() == "iterate.exe"}
    return next((w for w in Desktop(backend="uia").windows()
                 if w.process_id() in child_pids and w.is_visible()
                 and w.class_name() != "CASCADIA_HOSTING_WINDOW_CLASS"), None)


def editor(window):
    edits = [e for e in window.descendants(control_type="Edit")
             if e.is_visible() and e.is_enabled()]
    return max(edits, key=lambda e: e.rectangle().height()) if edits else None


def native_close(window):
    titlebar = window.descendants(control_type="TitleBar")[0]
    close = next(b for b in titlebar.descendants(control_type="Button")
                 if b.window_text() in ("关闭", "Close"))
    close.invoke()


def status(port):
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(
                f"http://127.0.0.1:{port}/status", timeout=1) as response:
            return json.load(response)
    except Exception:
        return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host")
    parser.add_argument("--mcp")
    parser.add_argument("--request")
    parser.add_argument("--evidence")
    parser.add_argument("--ports", nargs=2, type=int, default=[5581, 5582])
    args = parser.parse_args()
    if args.host:
        host(args.host)
        return
    mcp = Path(args.mcp).resolve(strict=True)
    exe = mcp.with_name("iterate.exe")
    saved_path = Path(args.request).resolve(strict=True)
    saved = json.loads(saved_path.read_text(encoding="utf-8-sig"))
    evidence = Path(args.evidence).resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    for port in args.ports:
        import socket
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", port))
    report = {"result": "Blocked", "stage": "startup", "source_request": str(saved_path),
              "source_sha256": hashlib.sha256(saved_path.read_bytes()).hexdigest(),
              "binaries": {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (mcp, exe)},
              "scope": "Real MCP stdio + native Windows console close; Python MCP host, not Codex CLI",
              "isolation": "Saved message/options preserved; workspace/title and WebView data directory isolated",
              "sessions": [], "observations": []}
    owned = {}
    windows = []
    terminals = []

    def remember(process):
        try:
            owned[process.pid] = process.create_time()
        except psutil.NoSuchProcess:
            pass

    def snapshot(label):
        processes = []
        for process in psutil.process_iter(["pid", "ppid", "name", "cmdline", "create_time"]):
            data = process.info
            if data["pid"] in owned or (data["name"] or "").lower() == "msedgewebview2.exe" and str(evidence) in " ".join(data["cmdline"] or []):
                processes.append(data)
        observation = {"label": label, "time": time.time(), "processes": processes,
                       "popup_visible": [bool(ctypes.windll.user32.IsWindowVisible(w.handle)) for w in windows],
                       "status": [status(p) for p in args.ports]}
        report["observations"].append(observation)
        ImageGrab.grab().save(evidence / f"{label}.png")
        write_json(evidence / "result.json", report)

    try:
        for index, port in enumerate(args.ports):
            # Routing matches ancestor workspaces; repo-local test directories
            # can accidentally reuse an existing real session's service.
            workspace = Path(tempfile.mkdtemp(prefix=f"iterate-terminal-close-{index}-"))
            title = f"iterate-close-test-{index}-{uuid.uuid4().hex[:6]}"
            config = {"mcp": str(mcp), "port": port, "workspace": str(workspace), "title": title,
                      "env": {"WEBVIEW2_USER_DATA_FOLDER": str(evidence / "webview"),
                              "ITERATE_DIALOG_GUI_EXECUTABLE": str(exe)},
                      "arguments": {"message": saved["message"],
                                    "predefined_options": saved.get("predefined_options", []),
                                    "is_markdown": saved.get("is_markdown", True),
                                    "project_path": str(workspace), "conversation_title": title}}
            for key in ("state", "initialize", "response", "stderr"):
                config[key] = str(evidence / f"session-{index}-{key}.json")
            config_path = evidence / f"session-{index}.json"
            write_json(config_path, config)
            console = subprocess.Popen(["conhost.exe", sys.executable, str(Path(__file__).resolve()),
                                        "--host", str(config_path)])
            remember(psutil.Process(console.pid))
            wait_for(lambda: Path(config["state"]).exists())
            state = json.loads(Path(config["state"]).read_text(encoding="utf-8"))
            remember(psutil.Process(state["host_pid"]))
            for process in descendants(state["host_pid"]):
                remember(process)
            terminals.append(wait_for(lambda: terminal(title)))
            window = wait_for(lambda: popup(state["host_pid"]))
            windows.append(window)
            children = descendants(state["host_pid"])
            for process in children:
                remember(process)
                command = process.cmdline()
                if process.name().lower() == "iterate.exe" and "--serve" in command:
                    port = int(command[command.index("--port") + 1])
                    args.ports[index] = port
            wait_for(lambda: editor(window))
            wait_for(lambda: (status(port) or {}).get("interaction_phase") == "waiting_user")
            for process in descendants(state["host_pid"]):
                remember(process)
            report["sessions"].append({**state, "popup_pid": window.process_id(),
                                        "terminal_handle": terminals[-1].handle,
                                        "popup_handle": window.handle, "port": port, "title": title})
        for index, window in enumerate(windows):
            ctypes.windll.user32.SetWindowPos(window.handle, 0, 20 + index * 640, 50, 0, 0, 0x0015)
        snapshot("before-close")
        report["stage"] = "terminal_native_close"
        native_close(terminals[0])
        for delay in (1, 2, 7):
            time.sleep(delay)
            snapshot(f"after-close-{len(report['observations'])}")
        first = report["sessions"][0]
        report["terminal_closed"] = not bool(ctypes.windll.user32.IsWindow(terminals[0].handle))
        report["host_exited"] = not psutil.pid_exists(first["host_pid"])
        report["own_popup_closed"] = not bool(ctypes.windll.user32.IsWindowVisible(windows[0].handle))
        remaining = wait_for(lambda: editor(windows[1]), timeout=8)
        windows[1].set_focus()
        remaining.click_input()
        remaining.type_keys("terminal-close-isolation-proof", with_spaces=True)
        report["other_editor_value"] = remaining.get_value()
        report["other_popup_usable"] = "terminal-close-isolation-proof" in report["other_editor_value"]
        snapshot("other-popup-input")
        report["result"] = "Pass" if all(report[k] for k in (
            "terminal_closed", "host_exited", "own_popup_closed", "other_popup_usable")) else "Fail"
    except Exception:
        report["error"] = traceback.format_exc()
        ImageGrab.grab().save(evidence / "failure.png")
    finally:
        # Terminate only identified test-owned PIDs with matching creation times.
        # Do not close popup titlebars, which can have global-shutdown semantics.
        for pid, created in list(owned.items()):
            try:
                if psutil.Process(pid).create_time() == created:
                    for process in descendants(pid):
                        remember(process)
            except psutil.NoSuchProcess:
                pass
        for pid, created in reversed(list(owned.items())):
            try:
                process = psutil.Process(pid)
                if process.create_time() == created:
                    process.terminate()
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                pass
        for window in terminals:
            try:
                if ctypes.windll.user32.IsWindow(window.handle):
                    native_close(window)
            except Exception:
                pass
        write_json(evidence / "result.json", report)
        print(json.dumps({k: v for k, v in report.items() if k != "observations"}, ensure_ascii=False), flush=True)
    if report["result"] != "Pass":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
