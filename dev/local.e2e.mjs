#!/usr/bin/env node
// Terminal-only integration checks in an isolated temporary workspace.
import assert from 'node:assert/strict'
import { readFileSync, existsSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { startServer, until, root } from './harness.mjs'

let app
try {
  app = await startServer()
  const { call, home, log } = app
  const chief = (await call('team.state')).bots[0]
  const conversation = chief.conversation
  const items = async () => (await call('chat.page', { conversation })).items
  const idle = () => until('idle', async () => !(await call('team.state')).work.runs[conversation])
  async function start(lines, approve = true) {
    await idle()
    const old = new Set((await call('recovery.list')).map(i => i.id))
    await call('chat.send', { conversation, text: lines.map(([name, args]) => `TOOL ${name} ${JSON.stringify(args)}`).join('\n') })
    const receipt = (await call('recovery.list')).find(i => !old.has(i.id) && i.conversation === conversation)
    assert.ok(receipt, 'the requested command has its own durable execution record')
    return async () => until('tool result', async () => {
      const approvals = (await call('team.state')).work.approvals.filter(a => a.conversation === conversation)
      if (approve) for (const a of approvals) await call('approval.answer', { id: a.id, allow: true })
      return (await call('recovery.get', { id: receipt.id })).result?.text
    })
  }
  async function run(name, args, approve = true) { return await (await start([[name, args]], approve))() }
  assert.equal((await call('local.status')).enabled, false)
  assert.match(await run('local_run', { command: 'printf forbidden' }), /Local terminal is off|Tool mcp__teambot__local_run not found/)
  await call('settings.local-computer', { enabled: true })
  assert.equal(JSON.parse(readFileSync(join(home, 'team/local-computer.json'))).enabled, true)
  assert.equal((await call('team.state')).localComputer.shell, 'bash')
  const configResult = await run('local_status', {})
  assert.match(configResult, /"enabled":true/)
  const args = readFileSync(log, 'utf8').trim().split('\n').map(l => JSON.parse(l).args).findLast(a => a.includes('--mcp-config'))
  const system = args[args.indexOf('--system-prompt') + 1]
  assert.match(system, /优先用 local_\*/); assert.match(system, /绝不运行 sudo/)
  const config = JSON.parse(args[args.indexOf('--mcp-config') + 1]).mcpServers.teambot
  assert.equal(config.env.DOTBOT_LOCAL, '1')
  async function listed(enabled) {
    const child = spawn(join(root, 'target/debug/dotbot'), ['mcp'], { env: { ...process.env, ...config.env, DOTBOT_LOCAL: enabled ? '1' : '0' }, stdio: ['pipe', 'pipe', 'inherit'] })
    let output = ''; child.stdout.on('data', b => output += b)
    const exited = once(child, 'exit')
    try { child.stdin.end(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }) + '\n'); await exited; return JSON.parse(output).result.tools.map(t => t.name) }
    finally { if (child.exitCode === null) { child.kill(); await exited } }
  }
  const localNames = (await listed(true)).filter(name => name.startsWith('local_'))
  assert.deepEqual(localNames.sort(), ['local_status', 'local_release', 'local_run', 'local_send_file', 'local_receive_file'].sort())
  assert.ok(!(await listed(false)).includes('local_run'))
  console.log('ok   local access is opt-in, persisted and exposed to MCP/prompt only when enabled')

  await call('bot.update', { id: chief.id, permission: 'ask' })
  const target = join(home, 'work/result.txt')
  const done = await start([['local_run', { command: "printf 'local result' > result.txt; printf approved", timeout_s: 5 }]], false)
  const approval = await until('local approval', async () => (await call('team.state')).work.approvals.find(a => a.tool === 'local_run'))
  assert.equal(existsSync(target), false)
  await call('approval.answer', { id: approval.id, allow: true })
  assert.match(await done(), /approved/)
  assert.equal(readFileSync(target, 'utf8'), 'local result')
  await call('secret.set', { name: 'LOCAL_TEST', value: 'local-private-value-92817' })
  assert.match(await run('local_run', { command: 'printf "%s" "$DOTBOT_SECRET_LOCAL_TEST"' }), /\[key LOCAL_TEST\]/)
  assert.ok(!(await items()).some(i => JSON.stringify(i).includes('local-private-value-92817')))
  assert.match(await run('local_send_file', { path: target }), /Sent result.txt/)
  const media = (await items()).findLast(i => i.kind === 'file').src
  const received = await run('local_receive_file', { file: media })
  const localPath = received.match(/Local copy: (.+)/)[1]
  assert.equal(readFileSync(localPath, 'utf8'), 'local result')
  assert.match(await run('local_receive_file', { file: 'media/../state.json' }), /Invalid media reference/)
  const helper = await call('bot.create', { name: 'LocalHelper', role: 'Local file helper' })
  assert.match(await run('message_bot', { bot: helper.name, text: 'Inspect the attached reference', files: [media] }), /Sent to LocalHelper/)
  await until('delegated file visible', async () => (await call('chat.page', { conversation: `bot:${helper.id}` })).items.some(i => i.kind === 'file' && i.src === media))
  console.log('ok   approval precedes local changes; secrets, attachment delivery and local delegation work')

  await call('bot.update', { id: chief.id, permission: 'readonly' })
  assert.match(await run('local_run', { command: 'printf denied' }), /read-only/)
  assert.match(await run('local_status', {}), /"shell":"bash"/)
  await call('bot.update', { id: chief.id, permission: 'ask' })
  for (const name of ['local_screenshot', 'local_click', 'local_drag', 'local_type', 'local_key', 'local_scroll', 'local_open']) {
    assert.match(await run(name, {}), /Unknown local terminal tool|Tool mcp__teambot__local_\w+ not found/)
  }
  await assert.rejects(call('local.peek'), /Unknown method/)
  await call('local.pause', { paused: true })
  assert.match(await run('local_run', { command: 'printf paused' }), /paused/)
  assert.match(await run('local_status', {}), /"paused":true/)
  await call('local.pause', { paused: false })
  const custom = join(home, 'custom-work'); mkdirSync(custom)
  assert.match(await run('local_run', { command: 'pwd', cwd: custom }), new RegExp(custom))
  assert.match(await run('local_run', { command: 'printf failed >&2; exit 9' }), /Local command failed[\s\S]*failed/)
  assert.match(await run('local_run', { command: 'sleep 5', timeout_s: 1 }), /exceeded 1 seconds/)
  console.log('ok   read-only policy, pause, working directories, failures and timeout are enforced; GUI tools are absent')

  for (const action of ['pause', 'disable', 'stop']) {
    const marker = join(home, `started-${action}`), forbidden = join(home, `finished-${action}`)
    const completion = await start([['local_run', { command: `printf start > '${marker}'; sleep 3; printf bad > '${forbidden}'`, timeout_s: 10 }]])
    const approval = await until('cancellable approval', async () => (await call('team.state')).work.approvals.find(a => a.tool === 'local_run'))
    await call('approval.answer', { id: approval.id, allow: true })
    await until('command started', () => existsSync(marker))
    if (action === 'pause') await call('local.pause', { paused: true })
    if (action === 'disable') await call('settings.local-computer', { enabled: false })
    if (action === 'stop') await call('chat.stop', { conversation })
    if (action !== 'stop') assert.match(await completion(), /cancelled/)
    await idle()
    await new Promise(resolve => setTimeout(resolve, 3200))
    assert.equal(existsSync(forbidden), false)
    await call('settings.local-computer', { enabled: true })
    await call('local.pause', { paused: false })
  }
  assert.equal((await call('local.status')).owner, null)
  await call('settings.local-computer', { enabled: false })
  assert.equal((await call('team.state')).localComputer.enabled, false)
  console.log('ok   pause, disable and Stop cancel owned child processes and release the shared computer')
} finally {
  await app?.stop()
}
