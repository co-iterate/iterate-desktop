"""Real saved requests, isolated WebView2 crashes, and native popup acceptance.

Requires pywinauto, Pillow and psutil. Evidence includes local task data.
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
import traceback
import urllib.request

import psutil
from pywinauto import Desktop
from pywinauto.controls.hwndwrapper import InvalidWindowHandle
from pywinauto.findwindows import ElementNotFoundError


def wait(probe, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = probe()
        except (InvalidWindowHandle, ElementNotFoundError):
            value = None  # Native windows may disappear during crash/recreation.
        if value:
            return value
        time.sleep(.3)
    raise TimeoutError("Observable result not reached")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True)
    parser.add_argument("--requests", nargs=2, required=True)
    parser.add_argument("--evidence", required=True)
    parser.add_argument("--startup-timeout", action="store_true")
    args = parser.parse_args()
    exe = Path(args.exe).resolve(strict=True)
    evidence = Path(args.evidence).resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    report = {"result": "Blocked", "exe_sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
              "steps": [], "sources": []}
    servers = []
    owned = {}
    futures = []
    pool = concurrent.futures.ThreadPoolExecutor(2)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def get(port, route):
        try:
            return json.load(opener.open(f"http://127.0.0.1:{port}/{route}", timeout=2))
        except OSError:
            return None

    def post(port, payload):
        request = urllib.request.Request(f"http://127.0.0.1:{port}/api/dialog",
            data=json.dumps(payload).encode(), headers={"Content-Type": "application/json"})
        return json.load(opener.open(request, timeout=300))

    def remember(process):
        owned[process.pid] = process.create_time()

    def web_processes():
        result = []
        for process in psutil.process_iter(["name", "cmdline"]):
            if process.info["name"].lower() == "msedgewebview2.exe" and str(evidence / "webview") in " ".join(process.info["cmdline"] or []):
                remember(process)
                result.append(process)
        return result

    def popup(server, excluded=()):
        children = psutil.Process(server.pid).children()
        for process in children:
            remember(process)
        pids = {p.pid for p in children} - set(excluded)
        native = next((w for w in Desktop(backend="win32").windows()
                       if w.process_id() in pids and w.is_visible()
                       and w.window_text() == "iterate"), None)
        return Desktop(backend="uia").window(handle=native.handle).wrapper_object() if native else None

    def editor(window):
        edits = [e for e in window.descendants(control_type="Edit") if e.is_visible() and e.is_enabled()]
        return max(edits, key=lambda e: e.rectangle().height()) if edits else None

    def usable_popup(server, excluded=()):
        window = popup(server, excluded)
        return window if window and editor(window) else None

    def focus(window):
        for _ in range(10):
            window.set_focus()
            time.sleep(.2)
            if ctypes.windll.user32.GetForegroundWindow() == window.handle:
                return
        raise RuntimeError("Another window keeps taking foreground focus")

    def snapshot(label, windows):
        report["steps"].append({"step": label, "pids": [w.process_id() for w in windows]})
        for index, window in enumerate(windows):
            native = Desktop(backend="win32").window(handle=window.handle)
            native.move_window(x=40 + index * 40, y=40)
            focus(window)
            window.capture_as_image().save(evidence / f"{label}-{index}.png")
            (evidence / f"{label}-{index}-controls.json").write_text(json.dumps(
                [(e.element_info.control_type, e.window_text()) for e in window.descendants()],
                ensure_ascii=False, indent=2), encoding="utf-8")
        (evidence / "result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")

    try:
        env = {**os.environ, "ITERATE_DIALOG_GUI_EXECUTABLE": str(exe),
               "WEBVIEW2_USER_DATA_FOLDER": str(evidence / "webview"),
               "ITERATE_CONFIG_DIR": str(evidence / "config"),
               "ITERATE_CROSS_DEVICE_DIR": str(evidence / "cross-device"),
               "ITERATE_LOG_FILE": str(evidence / "popup.log")}
        ports = [5591, 5592]
        windows = []
        for port, source in zip(ports, args.requests):
            assert get(port, "health") is None, "Test port already in use"
            data = Path(source).read_bytes()
            saved = json.loads(data.decode("utf-8-sig"))
            report["sources"].append({"path": source, "sha256": hashlib.sha256(data).hexdigest()})
            server = subprocess.Popen([str(exe), "--serve", "--port", str(port)], env=env,
                creationflags=subprocess.CREATE_NO_WINDOW, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            servers.append(server)
            remember(psutil.Process(server.pid))
            wait(lambda: get(port, "health"))
            payload = {key: saved[key] for key in ("message", "is_markdown", "conversation_title",
                       "codex_home", "codex_thread_id", "codex_deeplink") if key in saved}
            payload.update(workspace=saved["project_path"], options=saved.get("predefined_options", []), force_popup=True)
            futures.append(pool.submit(post, port, payload))
            window = wait(lambda: usable_popup(server))
            windows.append(window)
        snapshot("before", windows)
        # A shared browser exit must recreate BOTH real request windows.
        old = [w.process_id() for w in windows]
        browsers = [p for p in web_processes() if not any(a.startswith("--type=") for a in p.cmdline())]
        assert len(browsers) == 1, f"Expected isolated shared browser, found {len(browsers)}"
        browsers[0].terminate()
        if args.startup_timeout:
            # Suspend a new, test-owned GUI before it can acknowledge readiness.
            # The real supervisor must reap it and recreate the same saved request.
            def new_child():
                for server in servers:
                    for process in psutil.Process(server.pid).children():
                        if process.pid not in old and process.name().lower() == "iterate.exe":
                            return process
                return None
            process = wait(new_child)
            remember(process)
            process.suspend()
            wait(lambda: not psutil.pid_exists(process.pid), timeout=25)
            report["steps"].append({"step": "startup-timeout-reaped", "pid": process.pid})
        def recovered(server):
            assert not any(f.done() for f in futures), "Recovery completed a pending request"
            return usable_popup(server, old)
        windows = [wait(lambda s=s: recovered(s)) for s in servers]
        for window in windows:
            wait(lambda: editor(window))
        assert not any(f.done() for f in futures), "Recovery completed a pending request"
        snapshot("browser-recovered", windows)
        # Renderer loss must also recreate only affected requests and keep both usable.
        renderers = [p for p in web_processes() if "--type=renderer" in p.cmdline()]
        assert renderers
        old = [w.process_id() for w in windows]
        renderers[0].terminate()
        wait(lambda: any(not psutil.pid_exists(pid) for pid in old))
        windows = [wait(lambda s=s: usable_popup(s)) for s in servers]
        for window in windows:
            edit = wait(lambda: editor(window))
            focus(window)
            edit.click_input()
            edit.type_keys("recovery-proof", with_spaces=True)
            assert "recovery-proof" in edit.get_value()
        assert not any(f.done() for f in futures)
        snapshot("renderer-recovered-input", windows)
        titlebar = windows[0].descendants(control_type="TitleBar")[0]
        next(b for b in titlebar.descendants(control_type="Button") if b.window_text() in ("关闭", "Close")).invoke()
        result = futures[0].result(timeout=30)
        assert not result.get("error"), result
        time.sleep(1)
        assert popup(servers[0]) is None, "User close incorrectly restarted a popup"
        assert not futures[1].done(), "Closing one request completed another"
        # Activate the surviving native window even if desktop focus handling
        # left it minimized; request survival must not depend on foreground state.
        Desktop(backend="win32").window(handle=windows[1].handle).restore()
        focus(windows[1])
        assert "recovery-proof" in wait(lambda: editor(windows[1])).get_value()
        report["steps"].append({"step": "normal-close-isolated"})
        # Repeated failures must terminate with an error instead of restarting forever.
        for attempt in range(4):
            current = wait(lambda: usable_popup(servers[1]))
            previous = current.process_id()
            browser = next(p for p in web_processes() if not any(a.startswith("--type=") for a in p.cmdline()))
            browser.terminate()
            wait(lambda: futures[1].done() or usable_popup(servers[1], [previous]))
            if futures[1].done():
                break
        result = futures[1].result(timeout=5)
        assert result.get("error"), "Retry limit did not produce an explicit failure"
        assert popup(servers[1]) is None
        report["steps"].append({"step": "retry-limit", "additional_failures": attempt + 1, "error": result["error"]})
        report["result"] = "Pass"
    except Exception:
        report["result"] = "Fail" if report["steps"] else "Blocked"
        report["error"] = traceback.format_exc()
    finally:
        for server in servers:
            try:
                for process in psutil.Process(server.pid).children(recursive=True):
                    remember(process)
            except psutil.NoSuchProcess:
                pass
        web_processes()
        for pid, created in reversed(list(owned.items())):
            try:
                process = psutil.Process(pid)
                if process.create_time() == created:
                    process.terminate()
            except psutil.NoSuchProcess:
                pass
        pool.shutdown(wait=False, cancel_futures=True)
        (evidence / "result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps(report, indent=2))
    return 0 if report["result"] == "Pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
