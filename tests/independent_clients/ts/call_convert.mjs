#!/usr/bin/env node
/**
 * Independent TypeScript/Node MCP client against a Control-backed Capability
 * Gateway Streamable HTTP endpoint. No shared package filesystem; only URL +
 * bearer token from the host process.
 */
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

const endpoint = process.env.A3S_GATEWAY_ENDPOINT;
const token = process.env.A3S_GATEWAY_TOKEN;
if (!endpoint || !token) {
  console.error("A3S_GATEWAY_ENDPOINT and A3S_GATEWAY_TOKEN are required");
  process.exit(2);
}

const transport = new StreamableHTTPClientTransport(new URL(endpoint), {
  requestInit: {
    headers: {
      Authorization: `Bearer ${token}`,
    },
  },
});
const client = new Client({ name: "independent-ts", version: "1.0.0" });
await client.connect(transport);

const listed = await client.listTools();
const names = (listed.tools ?? []).map((tool) => tool.name);
if (names.length !== 1 || names[0] !== "convert") {
  console.error(`unexpected tools: ${JSON.stringify(names)}`);
  process.exit(1);
}

const result = await client.callTool({
  name: "convert",
  arguments: { args: [] },
});
if (result.isError) {
  console.error(`call_tool failed: ${JSON.stringify(result)}`);
  process.exit(1);
}
const exitCode = result.structuredContent?.exitCode;
if (exitCode !== 0) {
  console.error(`unexpected exitCode: ${JSON.stringify(result.structuredContent)}`);
  process.exit(1);
}

await client.close();
console.log("ok");
