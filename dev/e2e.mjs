#!/usr/bin/env node
// Protocol integration checks with the native engine and a fake model service: no key, no external messages.
import { readFileSync, existsSync } from 'node:fs'
import { join } from 'node:path'
import assert from 'node:assert/strict'
import { createHmac } from 'node:crypto'
import { createServer } from 'node:http'
import { startServer, until } from './harness.mjs'

// A connection's MCP server: lists one tool and records the Authorization header it gets.
const mcpSeen = { auth: [], calls: [] }
const mcpServer = createServer(async (request, response) => {
  let raw = ''
  for await (const chunk of request) raw += chunk
  const body = JSON.parse(raw)
  mcpSeen.auth.push(request.headers.authorization)
  const result = { initialize: { protocolVersion: '2025-03-26', capabilities: { tools: {} }, serverInfo: { name: 'tracker', version: '1' } }, 'tools/list': { tools: [{ name: 'echo', description: 'Echo text', inputSchema: { type: 'object', properties: { text: { type: 'string' } } } }] } }[body.method]
  if (body.method === 'tools/call') mcpSeen.calls.push(body.params)
  if (body.id === undefined) { response.writeHead(202); return response.end() }
  response.writeHead(200, { 'content-type': 'application/json' })
  response.end(JSON.stringify({ jsonrpc: '2.0', id: body.id, result: result ?? { content: [{ type: 'text', text: `echoed ${body.params?.arguments?.text}` }] } }))
}).listen(0, '127.0.0.1')
await new Promise(resolve => mcpServer.once('listening', resolve))
mcpServer.unref()
const mcpUrl = `http://127.0.0.1:${mcpServer.address().port}/mcp`
const app = await startServer({ extraEnv: { DOTBOT_SKILL_DEFAULTS: '0' } })
const { home, log, base, cookie, call } = app
const events = [], results = []
const feedController = new AbortController()
let feedTask
async function step(name, fn) {
  try { await fn(); results.push(`ok   ${name}`) } catch (error) { results.push(`FAIL ${name}: ${error.message}`) }
}
const items = async conversation => (await call('chat.page', { conversation })).items
const lastBot = async conversation => (await items(conversation)).filter(item => item.kind === 'bot').at(-1)?.text

try {
  const feed = await fetch(`${base}/api/bot/feed`, { headers: { cookie }, signal: feedController.signal })
  feedTask = (async () => {
    const reader = feed.body.getReader(), decoder = new TextDecoder()
    let buffer = ''
    for (;;) {
      const { value, done } = await reader.read()
      if (done) return
      buffer += decoder.decode(value, { stream: true })
      for (let end = buffer.indexOf('\n\n'); end >= 0; end = buffer.indexOf('\n\n')) {
        const block = buffer.slice(0, end); buffer = buffer.slice(end + 2)
        const event = block.match(/^event: ?(.*)$/m)?.[1], data = block.match(/^data: ?(.*)$/m)?.[1]
        if (event && data) events.push({ event, data: JSON.parse(data) })
      }
    }
  })().catch(error => { if (!feedController.signal.aborted) results.push(`FAIL feed: ${error.message}`) })
  let team = await call('team.state')
  const chief = team.bots[0]
  await step('a new team starts with a Main Bot and the feed says hello', async () => {
    assert.equal(chief.name, 'Chief'); assert.ok(chief.main)
    await until('hello', () => events.some(e => e.event === 'hello' && e.data.protocol === 2))
  })
  await step('a message gets an answer, streamed and saved', async () => {
    await call('chat.send', { conversation: chief.conversation, text: 'hello there' })
    await until('answer', async () => (await lastBot(chief.conversation)) === 'Echo: hello there')
    assert.ok(events.some(e => e.event === 'chat.stream' && e.data.conversation === chief.conversation && e.data.text))
    assert.ok(events.some(e => e.event === 'work' && e.data.runs[chief.conversation]))
  })
  await step('the next turn resumes the same Claude session, with the Bot prompt and tools', async () => {
    await call('chat.send', { conversation: chief.conversation, text: 'again' })
    await until('second answer', async () => (await lastBot(chief.conversation)) === 'Echo: again')
    const runs = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args)
    const [first, second] = runs.slice(-2)
    const sessionId = first[first.indexOf('--session-id') + 1]
    assert.equal(second[second.indexOf('--resume') + 1], sessionId)
    assert.match(second[second.indexOf('--system-prompt') + 1], /你是 Chief，/)
    assert.match(second[second.indexOf('--system-prompt') + 1], /# 你的记忆/)
  })
  await step('remember writes the DS Bot memory file, without exposing tool calls in chat', async () => {
    await call('chat.send', { conversation: chief.conversation, text: 'TOOL remember {"text":"The user likes green tea"}' })
    await until('saved', async () => (await lastBot(chief.conversation))?.startsWith('Done: Saved to your memory'))
    const file = readFileSync(join(home, 'team/memory/bots', chief.id, 'MEMORY.md'), 'utf8')
    assert.match(file, /^# Memory\n\n- The user likes green tea \[source: chat .*; added: \d{4}-\d{2}-\d{2}\]\n\n## Index\n$/)
    assert.equal((await items(chief.conversation)).some(item => item.kind === 'tool'), false)
    const view = await call('memory.view', { scope: 'bot', id: chief.id })
    assert.equal(view.files[0].entries[0].text, 'The user likes green tea')
  })
  let writer
  await step('the user creates a Bot and the feed patches only it in', async () => {
    writer = await call('bot.create', { name: 'Writer', role: 'Writes', instructions: 'Write short.' })
    await until('patch', () => events.some(e => e.event === 'team.patch' && e.data.ops.some(op => op.path[0] === 'bots' && op.path[1] === writer.id)))
  })
  await step('message_bot reaches the other Bot and its reply comes back', async () => {
    await call('chat.send', { conversation: chief.conversation, text: 'TOOL message_bot {"bot":"Writer","text":"ping"}' })
    await until('Writer got it', async () => (await items(`bot:${writer.id}`)).some(item => item.kind === 'incoming' && item.text === '[Message from Chief]\nping'))
    const task = await until('Writer completed', async () => (await call('task.list')).find(t => t.target === writer.id && t.text === 'ping' && t.status === 'completed'))
    await until('reply returned normally', async () => (await items(chief.conversation)).some(i => i.kind === 'incoming' && i.text.startsWith('[Reply from Writer]\nEcho: ping')))
  })
  await step('an ordinary Bot creates and starts a collaborator whose result returns automatically', async () => {
    await call('chat.send', { conversation: `bot:${writer.id}`, text: 'TOOL create_bot {"name":"Reviewer","role":"Review","description":"Checks drafts","instructions":"Check evidence and report briefly.","task":"review ping"}' })
    const helper = await until('collaborator created', async () => Object.values(JSON.parse(readFileSync(join(home, 'team/state.json'), 'utf8')).bots).find(bot => bot.name === 'Reviewer'))
    assert.equal(helper.createdBy ?? helper.created_by, writer.id)
    await until('helper started', async () => (await items(`bot:${helper.id}`)).some(item => item.kind === 'incoming' && item.text === '[Message from Writer]\nreview ping'))
    const task = await until('helper completed', async () => (await call('task.list')).find(t => t.target === helper.id && t.text === 'review ping' && t.status === 'completed'))
    await until('helper returned normally', async () => (await items(`bot:${writer.id}`)).some(i => i.kind === 'incoming' && i.text.startsWith('[Reply from Reviewer]\nEcho: review ping')))
    assert.equal(events.some(e => e.event === 'chat.item' && e.data.item?.kind === 'tool' && !e.data.item.name?.endsWith('__message_bot')), false, 'only messages to other Bots show as tool items')
  })
  let room
  await step('a group chat: the admin answers, @Name calls a member, and the log is DS Bot\'s', async () => {
    room = await call('room.create', { name: 'Launch', members: [chief.id, writer.id], model: 'unsupported/legacy-group-model' })
    assert.equal('model' in room, false)
    await call('chat.send', { conversation: `room:${room.id}`, text: 'Plan the launch' })
    await until('admin answered', async () => (await items(`room:${room.id}`)).some(item => item.who === 'Chief' && item.text.startsWith('Echo:')))
    await call('chat.send', { conversation: `room:${room.id}`, text: '@Writer draft the post' })
    await until('Writer answered', async () => (await items(`room:${room.id}`)).some(item => item.who === 'Writer' && item.text === 'Echo: User: @Writer draft the post'))
    const state = JSON.parse(readFileSync(join(home, 'team/state.json'), 'utf8'))
    const lines = state.rooms[room.id].log
    assert.deepEqual(lines.map(line => line.who), ['User', 'Chief', 'User', 'Writer'])
    assert.equal(lines[3].botId, writer.id)
  })
  await step('group members keep their Bot models; legacy overrides are ignored and Bot model changes apply', async () => {
    await call('bot.update', { id: chief.id, model: 'model-b' })
    await call('bot.update', { id: writer.id, model: 'model-c' })
    await call('room.update', { id: room.id, mode: 'all' })
    const groupRuns = () => readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args).filter(args => args.includes('--system-prompt') && args[args.indexOf('--system-prompt') + 1].includes('# 当前群聊'))
    const send = async text => {
      const start = groupRuns().length
      await call('chat.send', { conversation: `room:${room.id}`, text: `@all\n${text}` })
      await until(text, async () => {
        const replies = await items(`room:${room.id}`)
        return ['Chief', 'Writer'].every(name => replies.some(item => item.who === name && item.text.includes(text)))
      })
      return groupRuns().slice(start)
    }
    const assertModels = (runs, chiefModel) => {
      assert.equal(runs.length, 2)
      for (const [i, model] of [chiefModel, 'model-c'].entries()) assert.equal(runs[i][runs[i].indexOf('--model') + 1], model)
    }
    assertModels(await send('member models check'), 'model-b')
    await call('room.update', { id: room.id, model: 'unsupported/legacy-group-model' })
    const resumed = await send('ignore group override check')
    assertModels(resumed, 'model-b')
    assert.ok(resumed.every(run => run.includes('--resume')))
    const state = await call('team.state')
    assert.equal('model' in state.rooms.find(r => r.id === room.id), false)
    assert.equal(state.bots.find(b => b.id === chief.id).model, 'model-b')
    assert.equal(state.bots.find(b => b.id === writer.id).model, 'model-c')
    await call('bot.update', { id: chief.id, model: 'model-a' })
    const changed = await send('updated Bot model check')
    assertModels(changed, 'model-a')
    assert.ok(changed[0].includes('--session-id'))
    assert.equal(changed[0].includes('--resume'), false)
    assert.ok(changed[1].includes('--resume'))
    await call('bot.update', { id: chief.id, model: '' })
    await call('bot.update', { id: writer.id, model: '' })
    await call('room.update', { id: room.id, mode: 'admin' })
  })
  await step('a member writes the group memory from the group', async () => {
    // The fake reads TOOL at the start of a line; the member prompt puts "User: " before the first.
    const prompt = `TOOL remember {"text":"Launch is on Friday"}`
    await call('chat.send', { conversation: `room:${room.id}`, text: `@Chief\n${prompt}` })
    await until('group memory', () => existsSync(join(home, 'team/memory/groups', room.id, 'MEMORY.md')))
    assert.match(readFileSync(join(home, 'team/memory/groups', room.id, 'MEMORY.md'), 'utf8'), /- Launch is on Friday \[source: group "Launch"/)
  })
  await step('stopping a group cancels its entire round and preserves independent Bot work', async () => {
    const control = await call('room.create', { name: 'Stop controls', members: [chief.id, writer.id] })
    const key = `room:${control.id}`
    await call('room.update', { id: control.id, mode: 'all' })
    await call('chat.send', { conversation: `bot:${writer.id}`, text: 'DELAY 900\nindependent stop control' })
    await call('chat.send', { conversation: key, text: '@all Group stop control\nDELAY 5000\ncancel this round' })
    await until('group is active', async () => (await call('team.state')).work.runs[`${key}:${chief.id}`])
    assert.ok((await call('team.state')).work.runs[key], 'group stays busy for the whole round')
    assert.equal(await call('chat.stop', { conversation: key }), true)
    await until('group is idle', async () => !Object.keys((await call('team.state')).work.runs).some(k => k === key || k.startsWith(`${key}:`)))
    assert.equal((await items(key)).some(item => item.kind === 'bot'), false, 'no later member answers the stopped round')
    await until('independent Bot completes', async () => (await lastBot(`bot:${writer.id}`)) === 'Echo: independent stop control')
    const checkpoint = (await call('recovery.list')).find(w => w.conversation === key)
    assert.equal(checkpoint.status, 'cancelled')
    assert.equal(checkpoint.result.cancelled, true)
    await call('chat.send', { conversation: key, text: '@all new round after stopping' })
    await until('new round completes', async () => ['Chief', 'Writer'].every(name => JSON.parse(readFileSync(join(home, 'team/state.json'), 'utf8')).rooms[control.id].log.some(line => line.who === name && line.text.includes('new round after stopping'))))
  })
  await step('group stream failures retry with the same Bot model and session, and retain bounded failures', async () => {
    const retryBot = await call('bot.create', { name: 'Stream recovery', role: 'Recovery', model: 'model-c' })
    const retryRoom = await call('room.create', { name: 'Stream recovery group', members: [retryBot.id] })
    const key = `room:${retryRoom.id}`
    const runs = () => readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args).filter(args => args.includes('--system-prompt') && args[args.indexOf('--system-prompt') + 1].includes('# 当前群聊'))
    let start = runs().length
    await call('chat.send', { conversation: key, text: 'TRANSIENT once recover-once\nrecovered stream result' })
    await until('transient failure recovered', async () => (await items(key)).some(item => item.kind === 'bot' && item.text.includes('recovered stream result')))
    const attempts = runs().slice(start)
    assert.equal(attempts.length, 2)
    assert.ok(attempts.every(args => args[args.indexOf('--model') + 1] === 'model-c'))
    assert.equal(attempts[1][attempts[1].indexOf('--resume') + 1], attempts[0][attempts[0].indexOf('--session-id') + 1])
    assert.equal((await items(key)).some(item => item.error), false)
    start = runs().length
    await call('chat.send', { conversation: key, text: 'TRANSIENT always exhausted-retries\npermanent stream failure' })
    const failure = await until('retry limit reached', async () => (await items(key)).find(item => item.error && item.streamError))
    assert.equal(runs().slice(start).length, 3)
    assert.equal(failure.retryCount, 2)
    assert.match(failure.partial, /未完成的临时回复/)
    assert.equal((await items(key)).filter(item => item.kind === 'bot').length, 1, 'partial failures never become complete group answers')
    await until('failed group settles', async () => !Object.keys((await call('team.state')).work.runs).some(k => k === key || k.startsWith(`${key}:`)))

    const toolRoom = await call('room.create', { name: 'Stream after action', members: [retryBot.id] })
    const toolKey = `room:${toolRoom.id}`
    start = runs().length
    await call('chat.send', { conversation: toolKey, text: 'TRANSIENT after-tool keep-actions\nTOOL remember {"text":"stream action happens once"}' })
    const toolFailure = await until('tool failure kept for review', async () => (await items(toolKey)).find(item => item.error && item.streamError))
    assert.equal(toolFailure.usedTools, true)
    assert.equal(toolFailure.retryCount, 0)
    assert.equal(runs().slice(start).length, 1)
    assert.equal((readFileSync(join(home, 'team/memory/groups', toolRoom.id, 'MEMORY.md'), 'utf8').match(/stream action happens once/g) || []).length, 1)

    const stopRoom = await call('room.create', { name: 'Stop reconnecting', members: [retryBot.id] })
    const stopKey = `room:${stopRoom.id}`
    start = runs().length
    await call('chat.send', { conversation: stopKey, text: 'TRANSIENT always stop-reconnecting\nstop the retry' })
    await until('retry backoff', async () => (await call('team.state')).work.runs[`${stopKey}:${retryBot.id}`]?.activity === 'reconnecting')
    await call('chat.stop', { conversation: stopKey })
    await until('retry stopped', async () => !Object.keys((await call('team.state')).work.runs).some(k => k === stopKey || k.startsWith(`${stopKey}:`)))
    assert.equal(runs().slice(start).length, 1)
    assert.equal((await call('recovery.list')).find(w => w.conversation === stopKey).status, 'cancelled')
  })
  await step('a connection tool waits for the user\'s approval, then runs through the engine', async () => {
    await call('connection.save', { name: 'Tracker', kind: 'http', url: mcpUrl, headers: {} })
    await call('chat.send', { conversation: `bot:${writer.id}`, text: 'TOOL mcp__tracker__echo {"text":"approved call"}' })
    const approval = await until('approval', async () => (await call('team.state')).work.approvals[0])
    assert.equal(approval.tool, 'mcp__tracker__echo'); assert.equal(approval.bot, writer.id)
    assert.equal(mcpSeen.calls.length, 0, 'nothing runs before the user answers')
    await call('approval.answer', { id: approval.id, allow: true })
    await until('allowed', async () => (await lastBot(`bot:${writer.id}`))?.includes('echoed approved call'))
    assert.deepEqual(mcpSeen.calls.map(c => c.name), ['echo'])
  })
  await step('ordinary Bots still cannot change other Bots', async () => {
    await call('chat.send', { conversation: `bot:${writer.id}`, text: 'TOOL update_bot {"bot":"Chief","instructions":"changed"}' })
    await until('refused', async () => (await lastBot(`bot:${writer.id}`))?.includes('Only a Main Bot changes other Bots'))
  })
  const writerChat = () => `bot:${writer.id}`
  const writerMemory = () => readFileSync(join(home, 'team/memory/bots', writer.id, 'MEMORY.md'), 'utf8')
  await step('the memory review keeps what the user said, through Claude Code, after the chat goes quiet', async () => {
    await call('chat.send', { conversation: writerChat(), text: 'remember: The user is vegetarian' })
    await until('reviewed', () => existsSync(join(home, 'team/memory/bots', writer.id, 'MEMORY.md')) && writerMemory().includes('The user is vegetarian'))
    assert.match(writerMemory(), /- The user is vegetarian \[source: memory review .*\]/)
    await until('automatic review journal committed', () => readFileSync(join(home, 'team/memory/journal.jsonl'), 'utf8').split('\n').some(line => line.includes('"auto":true') && line.includes(writer.id) && line.includes('The user is vegetarian')))
    const oneShot = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args).find(args => args.includes('--json-schema'))
    assert.ok(oneShot && !oneShot.includes('--resume'), 'background jobs are one-shot structured requests')
  })
  await step('the memory panel answers and changes memory through the model', async () => {
    await assert.rejects(call('memory.summary', { scope: 'bot', id: writer.id }), /Unknown method/)
    const asked = await call('memory.ask', { scope: 'bot', id: writer.id, text: 'remember: Likes jazz' })
    assert.equal(asked.reply, 'Noted 1.')
    assert.match(writerMemory(), /- Likes jazz \[source: user /)
  })
  await step('ask_user opens a card; the answer goes on as the user\'s message', async () => {
    await call('chat.send', { conversation: writerChat(), text: 'TOOL ask_user {"question":"Which plan?","options":["Plan A","Plan B"]}' })
    const card = await until('card', async () => (await call('team.state')).work.cards.find(c => c.kind === 'question'))
    assert.deepEqual(card.options, ['Plan A', 'Plan B'])
    await call('card.answer', { id: card.id, answer: 'Plan B' })
    await until('answered', async () => (await lastBot(writerChat())) === 'Echo: Plan B')
  })
  await step('request_secret: the key goes into the store and the shell, never into the chat', async () => {
    await call('chat.send', { conversation: writerChat(), text: 'TOOL request_secret {"name":"demo key","purpose":"testing"}' })
    const card = await until('key card', async () => (await call('team.state')).work.cards.find(c => c.kind === 'secret'))
    assert.equal(card.name, 'DEMO_KEY')
    await call('card.answer', { id: card.id, answer: 'value-9f8e7d6c' })
    await until('told', async () => (await items(writerChat())).some(i => i.kind === 'incoming' && i.text.startsWith('[Key saved]')))
    await call('chat.send', { conversation: writerChat(), text: 'ENV DOTBOT_SECRET_DEMO_KEY' })
    await until('in env', async () => (await lastBot(writerChat())) === 'DOTBOT_SECRET_DEMO_KEY=set')
    assert.deepEqual((await call('team.state')).secrets, ['DEMO_KEY'])
    const { readdirSync } = await import('node:fs')
    for (const file of readdirSync(join(home, 'team/chats'))) assert.ok(!readFileSync(join(home, 'team/chats', file), 'utf8').includes('value-9f8e7d6c'))
  })
  await step('save_skill writes a Claude Code plugin skill that every turn loads', async () => {
    await call('chat.send', { conversation: writerChat(), text: 'TOOL save_skill {"name":"Weekly report","description":"Use for the weekly report","steps":"1. Gather\\n2. Write"}' })
    await until('skill', () => existsSync(join(home, 'team/plugin/skills/weekly-report/SKILL.md')))
    assert.ok((await call('team.state')).skills.some(skill => skill.name === 'weekly-report'))
    await call('chat.send', { conversation: writerChat(), text: 'next' })
    await until('next', async () => (await lastBot(writerChat())) === 'Echo: next')
    const args = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args).filter(a => !a.includes('--json-schema')).at(-1)
    assert.equal(args[args.indexOf('--plugin-dir') + 1], join(home, 'team/plugin'))
  })
  await step('skills are discoverable, conflicts cannot overwrite newer work, and disabled skills stay out of the plugin', async () => {
    const skill = await call('skill.get', { name: 'weekly-report' })
    await call('chat.send', { conversation: writerChat(), text: 'TOOL list_skills {"query":"weekly"}' })
    const catalog = await until('skill catalog tool', async () => {
      const text = await lastBot(writerChat())
      return text?.startsWith('Done: [') && text.includes('weekly-report') ? JSON.parse(text.slice(6)) : null
    })
    assert.equal('body' in catalog[0], false)
    await call('chat.send', { conversation: writerChat(), text: 'TOOL read_skill {"name":"weekly-report"}' })
    await until('full skill tool', async () => (await lastBot(writerChat()))?.includes(skill.revision))
    await call('chat.send', { conversation: writerChat(), text: `TOOL save_skill ${JSON.stringify({ name: skill.name, description: skill.description, steps: 'Verified team workflow v2', revision: skill.revision })}` })
    // A workflow the Bot wrote is patched in place (revision-checked), with no candidate.
    await until('Bot-authored skill patched', async () => (await call('skill.get', { name: skill.name })).body.includes('Verified team workflow v2'))
    assert.equal((await call('skill.candidates', { name: skill.name })).filter(c => c.kind === 'bot-update').length, 0)
    const updated=await call('skill.get',{name:skill.name})
    assert.notEqual(updated.revision, skill.revision)
    await assert.rejects(call('skill.save', { name: skill.name, description: 'outdated', text: 'lost update', revision: skill.revision }), /Skill changed/)
    await assert.rejects(call('skill.save', { name: skill.name, description: 'unversioned', text: 'lost update' }), /Skill changed/)
    await call('skill.toggle', { name: skill.name, enabled: false, revision: updated.revision })
    assert.equal(existsSync(join(home, 'team/plugin/skills/weekly-report/SKILL.md')), false)
    await call('chat.send', { conversation: writerChat(), text: `TOOL save_skill ${JSON.stringify({ name: skill.name, description: skill.description, steps: 'Bot edit of a disabled skill', revision: (await call('skill.get', { name: skill.name })).revision })}` })
    await until('disabled skill refused', async () => /disabled/i.test(await lastBot(writerChat()) ?? ''))
    assert.ok(!(await call('skill.get', { name: skill.name })).body.includes('Bot edit of a disabled skill'))
    await call('bot.update', { id: writer.id, permission: 'readonly' })
    await call('chat.send', { conversation: writerChat(), text: 'TOOL list_skills {"query":"weekly"}' })
    await until('disabled skill excluded', async () => (await lastBot(writerChat())) === 'Done: []')
    await call('chat.send', { conversation: writerChat(), text: 'TOOL read_skill {"name":"weekly-report"}' })
    await until('readonly skill inspection', async () => (await lastBot(writerChat()))?.includes('"enabled":false'))
    await call('bot.update', { id: writer.id, permission: 'auto' })
    await call('skill.save', { name: skill.name, description: updated.description, text: 'Verified disabled edit', revision: updated.revision })
    const disabled = await call('skill.get', { name: skill.name })
    assert.equal(disabled.enabled, false)
    await call('skill.toggle', { name: skill.name, enabled: true, revision: disabled.revision })
    assert.ok(readFileSync(join(home, 'team/plugin/skills/weekly-report/SKILL.md'), 'utf8').includes('Verified disabled edit'))
  })
  await step('a team connection reaches every Bot\'s turn, with its key filled in', async () => {
    const tracker = (await call('team.state')).connections.find(c => c.name === 'tracker')
    await call('connection.save', { ...tracker, headers: { Authorization: 'Bearer ${DEMO_KEY}' } })
    mcpSeen.auth.length = 0
    await call('chat.send', { conversation: writerChat(), text: 'again please' })
    await until('turn', async () => (await lastBot(writerChat())) === 'Echo: again please')
    assert.ok(mcpSeen.auth.length > 0 && mcpSeen.auth.every(value => value === 'Bearer value-9f8e7d6c'))
    await call('connection.save', { name: 'Offline', kind: 'http', url: 'http://127.0.0.1:9/mcp', headers: {} })
    await call('chat.send', { conversation: writerChat(), text: 'still answers' })
    await until('turn with an offline server', async () => (await lastBot(writerChat())) === 'Echo: still answers')
    const args = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args).filter(a => !a.includes('--json-schema')).at(-1)
    assert.match(args[args.indexOf('--system-prompt') + 1], /# 本轮不可用的 MCP 服务\n[^#]*- offline：/)
    await call('connection.delete', { name: 'offline' })
    assert.ok(readFileSync(join(home, 'team/connections.json'), 'utf8').includes('${DEMO_KEY}'))
  })
  await step('routines: a Bot sets a schedule, and a webhook brings its body to the Bot', async () => {
    await call('chat.send', { conversation: writerChat(), text: 'TOOL create_routine {"title":"Briefing","prompt":"Sum up the day","when":{"kind":"weekdays","time":"07:00"}}' })
    await until('routine', async () => (await call('routine.list')).some(r => r.title === 'Briefing' && r.schedule === 'weekdays at 07:00' && r.next))
    const hook = await call('routine.create', { title: 'On push', prompt: 'Check the push', target: writerChat(), when: { kind: 'webhook' } })
    const response = await fetch(`${base}/hook/${hook.id}/${hook.key}`, { method: 'POST', body: '{"ref":"main"}' })
    assert.equal(response.status, 200)
    assert.equal((await fetch(`${base}/hook/${hook.id}/wrong`, { method: 'POST', body: '' })).status, 404)
    await until('ran', async () => (await items(writerChat())).some(i => i.kind === 'incoming' && i.text.startsWith('[Routine "On push"]\nCheck the push') && i.text.includes('{"ref":"main"}')))
  })
  await step('pinning a Bot, and its description for the team', async () => {
    await call('bot.pin', { id: writer.id, pinned: true })
    await call('bot.update', { id: writer.id, description: 'Writes posts and emails' })
    const bot = (await call('team.state')).bots.find(b => b.id === writer.id)
    assert.equal(bot.pinned, true); assert.equal(bot.description, 'Writes posts and emails')
    await call('chat.send', { conversation: chief.conversation, text: 'TOOL list_bots {}' })
    await until('listed', async () => (await lastBot(chief.conversation))?.includes('For: Writes posts and emails'))
  })
  await step('the prompt declares actual capabilities and role templates are available', async () => {
    const team = await call('team.state')
    assert.equal(team.templates.length, 5)
    assert.ok(team.templates.every(t => t.description && t.instructions && t.zh.instructions))
    await call('settings.lang', {lang:'en'})
    await call('chat.send', { conversation: writerChat(), text: 'check environment' })
    await until('prompt turn', async () => (await lastBot(writerChat())) === 'Echo: check environment')
    const args = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args).filter(a => !a.includes('--json-schema')).at(-1)
    const system = args[args.indexOf('--system-prompt') + 1]
    assert.match(system, /桌面电脑：未开启/)
    assert.match(system, /独立浏览器：未开启/)
    assert.match(system, /所有用户可见的文字一律使用简体中文/)
    assert.ok(!system.includes('## 团队电脑'), 'desktop rules are only sent with a desktop')
    assert.ok(system.lastIndexOf('# 中文表达要求') > system.lastIndexOf('# 本机终端'))
    assert.ok(team.templates.every(t=>!t.instructions.includes('Simplified Chinese')&&!t.zh.instructions.includes('简体中文')), 'templates leave the language policy to language.md')
    await call('settings.lang', {lang:'zh'})
  })
  await step('a Bot model the provider rejects falls back to the default and the turn still answers', async () => {
    const bot = await call('bot.create', { name: 'Fallback', role: 'Tester', model: 'unsupported-model' })
    const chat = `bot:${bot.id}`
    await call('chat.send', { conversation: chat, text: 'model fallback check' })
    await until('fallback reply', async () => (await lastBot(chat)) === 'Echo: model fallback check')
    const notices = (await items(chat)).filter(i => i.kind === 'notice')
    assert.ok(notices.some(n => n.text.includes('unsupported-model is not supported') && !n.error), JSON.stringify(notices))
    assert.ok(!notices.some(n => n.error), 'the recovered turn must not show a model error')
    assert.equal((await call('team.state')).bots.find(b => b.id === bot.id).model, undefined)
  })
  await step('delegation is recorded with separate results and cycles are blocked', async () => {
    const cycle = `TOOL message_bot ${JSON.stringify({ bot: 'Chief', text: 'send this task back' })}`
    await call('chat.send', { conversation: chief.conversation, text: `TOOL message_bot ${JSON.stringify({ bot: 'Writer', text: cycle })}` })
    await until('cycle rejected', async () => (await call('task.list')).some(t => t.text === cycle && t.status === 'completed' && t.result.includes('Delegation loop')))
    assert.equal((await call('task.list')).some(t => t.text === 'send this task back'), false)
    await call('chat.send', { conversation: writerChat(), text: 'DELAY 1500\nblocking message' })
    await until('writer busy', async () => (await call('team.state')).work.runs[writerChat()])
    const reviewer = (await call('team.state')).bots.find(b => b.name === 'Reviewer')
    await Promise.all([
      call('chat.send', { conversation: chief.conversation, text: 'TOOL message_bot {"bot":"Writer","text":"isolated alpha"}' }),
      call('chat.send', { conversation: reviewer.conversation, text: 'TOOL message_bot {"bot":"Writer","text":"isolated beta"}' }),
    ])
    await until('separate completed tasks', async () => (await call('task.list')).filter(t => ['isolated alpha', 'isolated beta'].includes(t.text) && t.status === 'completed').length === 2)
    for (const text of ['isolated alpha', 'isolated beta']) assert.equal((await call('task.list')).find(t => t.text === text).result, `Echo: ${text}`)
  })
  await step('duplicate active delegation is reused and cancellation preserves other queued work', async () => {
    const tool = `TOOL message_bot ${JSON.stringify({ bot: 'Writer', text: 'DELAY 2500\nslow task' })}`
    await call('chat.send', { conversation: chief.conversation, text: `${tool}\n${tool}\nTOOL message_bot {"bot":"Writer","text":"keep queued task"}` })
    const slow = await until('slow active', async () => (await call('task.list')).find(t => t.text === 'DELAY 2500\nslow task' && t.status === 'running'))
    await until('next queued', async () => (await call('task.list')).some(t => t.text === 'keep queued task' && t.status === 'queued'))
    assert.equal((await call('task.list')).filter(t => t.text === slow.text).length, 1)
    await call('task.cancel', { id: slow.id })
    await until('next kept', async () => (await call('task.list')).some(t => t.text === 'keep queued task' && t.status === 'completed'))
    assert.equal((await call('task.get', { id: slow.id })).status, 'cancelled')
  })
  await step('structured delegation keeps context and lineage after asynchronous child results', async () => {
    const back = `TOOL message_bot ${JSON.stringify({ bot: 'Chief', text: 'forbidden return cycle' })}`
    const request = `TOOL message_bot ${JSON.stringify({ bot: 'Reviewer', text: 'DELAY 1200\nnested evidence' })}\nAFTER_CHILD ${back}`
    await call('chat.send', { conversation: chief.conversation, text: `TOOL message_bot ${JSON.stringify({ bot: 'Writer', text: request, context: 'Project: shared handoff', expected_output: 'Reviewed evidence', acceptance: 'Cite the observed source' })}` })
    const parent = await until('structured handoff', async () => (await call('task.list')).find(t => t.target === writer.id && t.text.includes('Project: shared handoff')))
    assert.match(parent.text, /Expected deliverable:\nReviewed evidence/)
    assert.match(parent.text, /Acceptance criteria:\nCite the observed source/)
    await until('returned child cycle rejected', async () => (await items(writerChat())).some(i => i.kind === 'incoming' && i.text.includes('Echo: nested evidence')) && (await lastBot(writerChat()))?.includes('Delegation loop'))
    assert.equal((await call('task.list')).some(t => t.text === 'forbidden return cycle'), false)
    const runs = readFileSync(log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args)
    assert.ok(runs.some(args => args[args.indexOf('--system-prompt') + 1]?.includes(`"parent":"${parent.id}"`)))
    await call('chat.send', { conversation: chief.conversation, text: `TOOL list_tasks ${JSON.stringify({ id: parent.id })}` })
    await until('receipt inspection', async () => (await lastBot(chief.conversation))?.startsWith('Done: [') && (await lastBot(chief.conversation))?.includes(parent.id))
  })
  await step('stopping a parent cancels its queued child without dropping an unrelated user request', async () => {
    const reviewer = (await call('team.state')).bots.find(b => b.name === 'Reviewer')
    await call('chat.send', { conversation: reviewer.conversation, text: 'DELAY 8000\noccupy only the child Bot' })
    await until('child Bot busy', async () => (await call('team.state')).work.runs[reviewer.conversation]?.acceptingCorrections)
    const occupied = (await items(reviewer.conversation)).find(i => i.kind === 'user' && i.text === 'DELAY 8000\noccupy only the child Bot')
    const request = `TOOL message_bot ${JSON.stringify({ bot: 'Reviewer', text: 'DELAY 6000\nchild must stop' })}\nWAIT_AFTER_TOOLS 8000`
    await call('chat.send', { conversation: chief.conversation, text: `TOOL message_bot ${JSON.stringify({ bot: 'Writer', text: request })}` })
    const parent = await until('running parent', async () => (await call('task.list')).find(t => t.text === request && t.status === 'running'))
    const child = await until('queued child', async () => {const t=(await call('task.list')).find(t => t.parent === parent.id && t.status === 'queued');return t&&((await call('team.state')).work.runs[t.targetConversation]?.queued||0)>0&&t})
    await call('chat.send', { conversation: reviewer.conversation, text: 'unrelated user request survives' })
    await call('task.cancel', { id: parent.id })
    await call('chat.stop_item', { conversation: reviewer.conversation, item: occupied.id })
    await until('unrelated request completed', async () => (await lastBot(reviewer.conversation)) === 'Echo: unrelated user request survives')
    assert.equal((await call('task.get', { id: child.id })).status, 'cancelled')
    assert.equal((await call('task.get', { id: parent.id })).status, 'cancelled')
  })
  await step('read-only is enforced on team tools and desktop commands', async () => {
    await call('bot.update', { id: writer.id, permission: 'readonly' })
    await call('chat.send', { conversation: writerChat(), text: 'TOOL create_bot {"name":"Forbidden","role":"test","instructions":"test"}\nTOOL desktop_run {"command":"touch forbidden"}' })
    await until('policy refused', async () => (await lastBot(writerChat()))?.includes('This Bot is read-only'))
    assert.equal((await call('team.state')).bots.some(b => b.name === 'Forbidden'), false)
    await call('bot.update', { id: writer.id, permission: 'auto' })
  })
  await step('secret values split across streamed chunks stay redacted', async () => {
    const start = events.length
    await call('chat.send', { conversation: writerChat(), text: 'value-9f8e7d6c' })
    await until('redacted reply', async () => (await lastBot(writerChat())) === 'Echo: [key DEMO_KEY]')
    await until('redacted stream reaches the feed reader', () => events.slice(start).filter(e => e.event === 'chat.stream' && e.data.conversation === writerChat()).map(e => e.data.text ?? '').join('') === 'Echo: [key DEMO_KEY]')
    const stream = events.slice(start).filter(e => e.event === 'chat.stream' && e.data.conversation === writerChat()).map(e => e.data.text ?? '').join('')
    assert.equal(stream, 'Echo: [key DEMO_KEY]')
    assert.ok(!JSON.stringify(events.slice(start)).includes('value-9f8e7d6c'))
  })
  await step('routines edit transactionally, retain timezones and keep completed run history', async () => {
    const routine = await call('routine.create', { title: 'Timezone check', prompt: 'timezone test', target: writerChat(), timezone: 'Asia/Shanghai', when: { kind: 'daily', time: '07:00' } })
    await assert.rejects(call('routine.update', { id: routine.id, title: 'bad edit', when: { kind: 'weekly', day: 7, time: '07:00' } }))
    assert.equal((await call('routine.list')).find(r => r.id === routine.id).title, 'Timezone check')
    await call('routine.update', { id: routine.id, prompt: 'updated timezone test', timezone: 'America/New_York', enabled: false })
    let saved = (await call('routine.list')).find(r => r.id === routine.id)
    assert.equal(saved.enabled, false); assert.equal(saved.timezone, 'America/New_York')
    assert.equal((await fetch(`${base}/hook/${routine.id}/${routine.key}`, { method: 'POST', body: '{}' })).status, 404)
    await call('routine.run', { id: routine.id })
    await until('manual run completed', async () => (await call('routine.history', { id: routine.id })).some(r => r.status === 'completed' && r.trigger === 'manual' && r.result.includes('updated timezone test')))
    await call('chat.send', { conversation: writerChat(), text: 'TOOL update_routine {"routine":"Timezone check","enabled":true}' })
    await until('bot resumed routine', async () => (await call('routine.list')).find(r => r.id === routine.id)?.enabled)
    const groupRoutine = await call('routine.create', { title: 'Group routine', prompt: 'group routine result', target: `room:${room.id}`, when: { kind: 'webhook' } })
    await call('routine.run', { id: groupRoutine.id })
    await until('group run completed', async () => (await call('routine.history', { id: groupRoutine.id })).some(r => r.status === 'completed' && r.result.includes('Chief:')))
  })
  await step('GitHub events verify signatures, filter, deduplicate and queue distinct events', async () => {
    await call('secret.set', { name: 'WEBHOOK_SIGNING', value: 'test-webhook-secret-12345' })
    const routine = await call('routine.create', { title: 'GitHub push', prompt: 'inspect event', target: writerChat(), when: { kind: 'event', source: 'github', event: 'push', filters: { '/ref': 'refs/heads/main' }, signingSecret: 'WEBHOOK_SIGNING' } })
    const body = JSON.stringify({ ref: 'refs/heads/main', after: 'abc' })
    const signature = `sha256=${createHmac('sha256', 'test-webhook-secret-12345').update(body).digest('hex')}`
    const post = async (id, event = 'push', sig = signature) => fetch(`${base}/hook/${routine.id}/${routine.key}`, { method: 'POST', headers: { 'x-hub-signature-256': sig, 'x-github-event': event, 'x-github-delivery': id }, body })
    assert.equal((await post('bad', 'push', 'sha256=00')).status, 404)
    assert.equal((await (await post('filtered', 'issues')).json()).accepted, false)
    assert.equal((await (await post('one')).json()).accepted, true)
    assert.equal((await (await post('one')).json()).accepted, false)
    assert.equal((await (await post('two')).json()).accepted, true)
    await until('event runs completed', async () => (await call('routine.history', { id: routine.id })).filter(r => r.status === 'completed').length === 2)
    assert.equal((await call('routine.history', { id: routine.id })).length, 2)
    assert.ok((await call('routine.list')).find(r => r.id === routine.id).last)
  })
  await step('a failed delegated turn is recorded as failed and its requester receives a result', async () => {
    await call('chat.send', { conversation: chief.conversation, text: 'TOOL message_bot {"bot":"Writer","text":"FAIL synthetic failure"}' })
    const task = await until('failure recorded', async () => (await call('task.list')).find(t => t.text === 'FAIL synthetic failure' && t.status === 'failed'))
    await until('failure returned normally', async () => (await items(chief.conversation)).some(i => i.kind === 'incoming' && /^\[Reply from Writer\]\nTask failed: .*synthetic failure/.test(i.text)))
  })
  await step('an image the user sends reaches Claude Code with the message, and shows in the chat', async () => {
    const png = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='
    await call('chat.send', { conversation: writerChat(), text: 'What is this?', images: [png] })
    await until('seen', async () => (await lastBot(writerChat()))?.startsWith('Saw 1 image(s): What is this?'))
    const sent = (await items(writerChat())).find(i => i.kind === 'user' && i.text === 'What is this?')
    assert.equal(sent.images.length, 1)
    assert.equal((await fetch(`${base}/${sent.images[0]}`, { headers: { cookie } })).headers.get('content-type'), 'image/png')
  })
  await step('deleting a Bot archives its memory and takes it out of groups', async () => {
    await call('bot.delete', { id: chief.id })
    team = await call('team.state')
    assert.deepEqual(team.mainBotIds, [writer.id])
    assert.deepEqual(team.rooms[0].members, [writer.id])
    assert.ok(!existsSync(join(home, 'team/memory/bots', chief.id)))
    assert.match(readFileSync(join(home, 'team/memory/journal.jsonl'), 'utf8'), /"op":"archive"/)
  })
} finally {
  feedController.abort()
  await feedTask
  await app.stop()
  console.log(results.join('\n'))
  const failed = results.filter(line => line.startsWith('FAIL')).length
  console.log(failed ? `\ne2e: ${failed} failed` : '\ne2e: all passed')
  process.exitCode = failed ? 1 : 0
}
