#!/usr/bin/env node
// A deterministic browser MCP server for transport/takeover/teaching checks.
// It launches no browser and never contacts external sites.
import { createServer } from 'node:http'
import { existsSync, readFileSync, writeFileSync, symlinkSync, unlinkSync } from 'node:fs'
import { join } from 'node:path'
import { hostname } from 'node:os'
const args = process.argv.slice(2)
const port = Number(args[args.indexOf('--port') + 1])
const tools = ['browser_snapshot', 'browser_take_screenshot', 'browser_navigate', 'browser_click', 'browser_evaluate', 'browser_tabs', 'browser_run_code_unsafe', 'browser_file_upload']
const profile = args[args.indexOf('--user-data-dir') + 1]
const lock = join(profile, 'SingletonLock')
try { symlinkSync(`${hostname()}-${process.pid}`, lock) } catch {}
const calls = join(profile, 'fixture-calls.jsonl')
const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='
const server = createServer(async (request, response) => {
  let raw = ''
  for await (const chunk of request) raw += chunk
  const message = JSON.parse(raw)
  if (!('id' in message)) { response.writeHead(202); response.end(); return }
  let result = {}
  if (message.method === 'initialize') result = { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'fake-browser', version: '1' } }
  if (message.method === 'tools/list') result = { tools: tools.map(name => ({ name, description: name, inputSchema: { type: 'object', properties: {} } })) }
  if (message.method === 'tools/call') {
    writeFileSync(calls, JSON.stringify({ name: message.params.name, args: message.params.arguments }) + '\n', { flag: 'a' })
    if (message.params.name === 'browser_take_screenshot') result = { content: [{ type: 'image', mimeType: 'image/png', data: process.env.FAKE_BROWSER_SCREENSHOT && existsSync(process.env.FAKE_BROWSER_SCREENSHOT) ? readFileSync(process.env.FAKE_BROWSER_SCREENSHOT).toString('base64') : png }] }
    else if (message.params.name === 'browser_tabs') result = { content: [{ type: 'text', text: '- 0: (current) [Example](https://example.com/)' }] }
    else if (message.params.name === 'browser_run_code_unsafe' && message.params.arguments?.code?.includes('storageState')) result = { content: [{ type: 'text', text: existsSync(join(profile, 'empty-cookies')) ? '### Result\n"[]"' : '### Result\n' + JSON.stringify(JSON.stringify([{ name: 'session', value: 'fixture-login', domain: 'example.com', path: '/', expires: -1, httpOnly: true, secure: true, sameSite: 'Lax' }])) }] }
    else if (message.params.arguments?.fail) result = { isError: true, content: [{ type: 'text', text: '### Error\nElement not found' }] }
    else result = { content: [{ type: 'text', text: '### Result\nDone' }] }
  }
  response.writeHead(200, { 'content-type': 'application/json', 'mcp-session-id': 'test-browser-session' })
  response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }))
})
server.listen(port, '127.0.0.1')
process.on('SIGTERM', () => { try { unlinkSync(lock) } catch {} server.close(() => process.exit(0)) })
