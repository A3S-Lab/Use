#!/usr/bin/env python3
"""Independent Python MCP client against a Control-backed Capability Gateway.

Uses only the public Streamable HTTP endpoint and bearer token; no shared
package filesystem with the Use host.
"""

from __future__ import annotations

import asyncio
import os
import sys

import httpx
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client


async def main() -> int:
    endpoint = os.environ.get("A3S_GATEWAY_ENDPOINT")
    token = os.environ.get("A3S_GATEWAY_TOKEN")
    if not endpoint or not token:
        print(
            "A3S_GATEWAY_ENDPOINT and A3S_GATEWAY_TOKEN are required",
            file=sys.stderr,
        )
        return 2

    headers = {"Authorization": f"Bearer {token}"}
    async with httpx.AsyncClient(headers=headers) as http_client:
        async with streamable_http_client(endpoint, http_client=http_client) as (
            read_stream,
            write_stream,
            _,
        ):
            async with ClientSession(read_stream, write_stream) as session:
                await session.initialize()
                listed = await session.list_tools()
                names = [tool.name for tool in listed.tools]
                if names != ["convert"]:
                    print(f"unexpected tools: {names!r}", file=sys.stderr)
                    return 1
                result = await session.call_tool(
                    "convert", arguments={"args": []}
                )
                if result.isError:
                    print(f"call_tool failed: {result!r}", file=sys.stderr)
                    return 1
                structured = result.structuredContent or {}
                if structured.get("exitCode") != 0:
                    print(
                        f"unexpected structured content: {structured!r}",
                        file=sys.stderr,
                    )
                    return 1
    print("ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
