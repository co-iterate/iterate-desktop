"""Exercise native titlebar X with two real saved popup requests on Windows.

Requires pywinauto, psutil, and Pillow. Only test-owned processes are cleaned up.
Build the executable with: cargo build --release --features custom-protocol --bin iterate
"""
import argparse
import concurrent.futures
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request
import uuid

import psutil
from PIL import ImageGrab
from pywinauto import Desktop


def wait_until(probe, seconds=45):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = probe()
        if result:
            return result
        time.sleep(0.2)
    raise TimeoutError("Expected observable state did not arrive")


def get_json(port, route):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/{route}", timeout=2) as response:
        return json.load(response)


def healthy(port):
    try:
        return get_json(port, "health")["status"] == "ok"
    except OSError:
        return False


def post_dialog(port, payload):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/dialog",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        return json.load(response)


def popup_for(server):
    children = {p.pid for p in psutil.Process(server.pid).children()}
    return next((w for w in Desktop(backend="uia").windows()
                 if w.process_id() in children and w.is_visible()), None)


def click_native_x(window):
    window.set_focus()
    # Invoke the actual native titlebar button through Windows accessibility.
    # This avoids another foreground window stealing coordinate-based input.
    titlebar = window.descendants(control_type="TitleBar")[0]
    close_button = next(button for button in titlebar.descendants(control_type="Button")
                        if button.window_text() in ("关闭", "Close"))
    close_button.invoke()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True)
    parser.add_argument("--requests", nargs=2, required=True)
    parser.add_argument("--evidence", required=True)
    parser.add_argument("--ports", nargs=2, type=int, default=[5418, 5419])
    args = parser.parse_args()
    exe = Path(args.exe).resolve(strict=True)
    evidence = Path(args.evidence).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    assert not any(healthy(port) for port in args.ports), "Test ports already in use"
    payloads = []
    for filename in args.requests:
        saved = json.loads(Path(filename).read_text(encoding="utf-8-sig"))
        payloads.append({
            "request_id": f"close-isolation-{uuid.uuid4()}",
            **{key: saved[key] for key in (
                "message", "is_markdown", "codex_home", "codex_thread_id",
                "codex_deeplink", "conversation_title") if key in saved},
            "workspace": saved["project_path"],
            "options": saved.get("predefined_options", []),
            "force_popup": True,
        })
    servers = []
    popup_pids = []
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=2)
    report = {"exe": str(exe), "sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
              "requests": args.requests, "result": "Blocked", "stage": "startup"}
    try:
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0
        env = {**os.environ, "ITERATE_DIALOG_GUI_EXECUTABLE": str(exe)}
        for port in args.ports:
            server = subprocess.Popen([str(exe), "--serve", "--port", str(port)],
                                      env=env, startupinfo=startup,
                                      stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL)
            servers.append(server)
            wait_until(lambda: healthy(port))
        futures = []
        windows = []
        for server, port, payload in zip(servers, args.ports, payloads):
            futures.append(pool.submit(post_dialog, port, payload))
            window = wait_until(lambda: popup_for(server))
            windows.append(window)
            popup_pids.append(window.process_id())
            wait_until(lambda: get_json(port, "status")["interaction_phase"] == "waiting_user")
        report["popup_pids"] = popup_pids
        for index, window in enumerate(windows):
            ctypes.windll.user32.SetWindowPos(window.handle, 0, 30 + index * 650, 40, 0, 0, 0x0015)
        report.update(result="Fail", stage="native_titlebar_close")
        windows[0].set_focus()
        time.sleep(1)
        ImageGrab.grab().save(evidence / "before-close.png")
        click_native_x(windows[0])
        first = futures[0].result(timeout=15)
        report["first_response"] = first
        assert first.get("keep_going") is False and not first.get("error"), first
        assert not psutil.pid_exists(popup_pids[0]), "First popup still running"
        assert psutil.pid_exists(popup_pids[1]), "Second popup exited"
        assert not futures[1].done(), "Second request was cancelled"
        assert all(healthy(port) for port in args.ports), "A server exited"
        remaining = windows[1]
        remaining.set_focus()
        edits = wait_until(lambda: [e for e in remaining.descendants(control_type="Edit")
                                   if e.is_visible() and e.is_enabled()])
        # The multiline reply editor is taller than the title/template search fields.
        editor = max(edits, key=lambda edit: edit.rectangle().height())
        editor.click_input()
        editor.type_keys("close-isolation-proof", with_spaces=True)
        observed = editor.get_value()
        assert "close-isolation-proof" in observed, observed
        report["remaining_editor_value"] = observed
        ImageGrab.grab().save(evidence / "after-close-other-editable.png")
        editor.type_keys("^a{BACKSPACE}")
        click_native_x(remaining)
        second = futures[1].result(timeout=15)
        report["second_response"] = second
        assert second.get("keep_going") is False and not second.get("error"), second
        assert all(healthy(port) for port in args.ports), "Closing last popup killed a server"
        report["result"] = "Pass"
    except Exception as error:
        report["error"] = repr(error)
        ImageGrab.grab().save(evidence / "failure.png")
        raise
    finally:
        (evidence / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        for server in servers:
            if server.poll() is None:
                for child in psutil.Process(server.pid).children():
                    if Path(child.exe()).resolve() == exe:
                        child.terminate()
                server.terminate()
                server.wait(timeout=5)
        pool.shutdown(wait=False, cancel_futures=True)
        print(json.dumps(report, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
