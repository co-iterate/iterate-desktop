#!/usr/bin/env python3
"""Merge the Iterate MCP entry into Codex config without dropping user settings."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

TIMEOUT_SECONDS = 315_360_000
TARGET_HEADERS = (
    '[mcp_servers."iterate-zhi"]',
    "[mcp_servers.iterate-zhi]",
    "[mcp_servers.cunzhi]",
)
TOOL_HEADER_PATTERN = re.compile(
    r'(?m)^\[(?:mcp_servers\."iterate-zhi"|mcp_servers\.iterate-zhi|mcp_servers\.cunzhi)\.tools\.call_zhi\]\s*$'
)


def find_section(text: str) -> tuple[int, int, str] | None:
    header_pattern = r"(?m)^\[(?:mcp_servers\.\"iterate-zhi\"|mcp_servers\.iterate-zhi|mcp_servers\.cunzhi)\]\s*$"
    match = re.search(header_pattern, text)
    if not match:
        return None

    next_header = re.search(r"(?m)^\[", text[match.end() :])
    end = match.end() + next_header.start() if next_header else len(text)
    return match.start(), end, match.group(0).strip()


def replace_setting(block: str, name: str, value: str) -> str:
    pattern = rf"(?m)^\s*{re.escape(name)}\s*=.*$"
    replacement = f"{name} = {value}"
    if re.search(pattern, block):
        return re.sub(pattern, replacement, block, count=1)
    return block


def tool_header_for_server(server_header: str) -> str:
    return server_header[:-1] + ".tools.call_zhi]"


def append_default_tool_approval(text: str, server_header: str, insertion: int) -> str:
    tool_match = TOOL_HEADER_PATTERN.search(text)
    if tool_match:
        next_header = re.search(r"(?m)^\[", text[tool_match.end() :])
        end = tool_match.end() + next_header.start() if next_header else len(text)
        block = text[tool_match.start() : end]
        if re.search(r"(?m)^\s*approval_mode\s*=", block):
            return text
        block = block.rstrip("\n") + '\napproval_mode = "approve"\n'
        return text[: tool_match.start()] + block + text[end:]

    before = text[:insertion].rstrip("\n")
    after = text[insertion:].lstrip("\n")
    tool_block = (
        f'{tool_header_for_server(server_header)}\n'
        'approval_mode = "approve"\n'
    )
    suffix = f"\n{after}" if after else ""
    return f"{before}\n\n{tool_block}{suffix}"


def merge_config(text: str, command: str) -> str:
    section = find_section(text)
    command_value = json.dumps(command, ensure_ascii=False)

    if section is None:
        suffix = "" if not text or text.endswith("\n") else "\n"
        if text.strip():
            suffix += "\n"
        server_header = '[mcp_servers."iterate-zhi"]'
        merged = (
            text
            + suffix
            + f'{server_header}\n'
            + f"command = {command_value}\n"
            + "args = []\n"
            + f"tool_timeout_sec = {TIMEOUT_SECONDS}\n"
        )
        return append_default_tool_approval(merged, server_header, len(merged))

    start, end, _ = section
    block = text[start:end]
    block = replace_setting(block, "command", command_value)
    if not re.search(r"(?m)^\s*args\s*=", block):
        command_end = re.search(r"(?m)^\s*command\s*=.*$", block)
        insertion = command_end.end() if command_end else block.find("\n")
        block = block[:insertion] + "\nargs = []" + block[insertion:]
    block = replace_setting(block, "tool_timeout_sec", str(TIMEOUT_SECONDS))
    if not re.search(r"(?m)^\s*tool_timeout_sec\s*=", block):
        block = block.rstrip("\n") + f"\ntool_timeout_sec = {TIMEOUT_SECONDS}\n"

    merged = text[:start] + block + text[end:]
    insertion = start + len(block)
    return append_default_tool_approval(merged, section[2], insertion)


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {Path(sys.argv[0]).name} CONFIG_PATH MCP_SERVER_PATH", file=sys.stderr)
        return 2

    path = Path(sys.argv[1]).expanduser()
    command = sys.argv[2]
    text = path.read_text() if path.exists() else ""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(merge_config(text, command))
    print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
