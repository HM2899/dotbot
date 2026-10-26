#!/usr/bin/env node
import assert from 'node:assert/strict'
import { readFileSync, writeFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until } from './harness.mjs'

let app
let home
const runs = () => readFileSync(app.log, 'utf8').trim().split('\n').filter(Boolean).map(x => JSON.parse(x).args).filter(a => a.includes('--input-format'))
const items = async key => (await app.call('chat.page', { conversation: key })).items
const idle = async () => until('group settles', async () => Object.keys((await app.call('team.state')).work.runs).length === 0)
try {
  app = await startServer({ keepHome: true }); home = app.home
  const { call } = app
  const chief = (await call('team.state')).bots[0]
  const bot = await call('bot.create', { name: 'bot1', role: 'Tester', model: 'model-b' })
  const room = await call('room.create', { name: '项目讨论', members: [bot.id] })
  const key = `room:${room.id}`, seat = `${key}:${bot.id}`
  await call('sidebar.group.create', { name: '项目 A', conversations: [`bot:${bot.id}`, key] })
  let state = await call('team.state')
  assert.deepEqual(state.sidebarGroups, ['项目 A'])
  assert.equal(state.bots.find(b => b.id === bot.id).sidebarGroup, '项目 A')
  assert.equal(state.rooms[0].sidebarGroup, '项目 A')
  assert.equal(state.bots.find(b => b.id === bot.id).model, 'model-b')
  await assert.rejects(call('sidebar.group.create', { name: '项目 A' }))
  await assert.rejects(call('sidebar.group.create', { name: ' '.repeat(3) }))
  await assert.rejects(call('sidebar.group.update', { name: '项目 A', newName: '不应保存', conversations: ['bot:missing'] }))
  assert.deepEqual((await call('team.state')).sidebarGroups, ['项目 A'])
  await call('sidebar.group.update', { name: '项目 A', newName: '研发', collapsed: true })
  assert.deepEqual((await call('team.state')).prefs.sidebarCollapsed, ['研发'])
  await call('sidebar.group.create', { name: '<工作>&"' })
  await call('sidebar.move', { conversation: key, group: '<工作>&"' })
  await call('sidebar.group.delete', { name: '<工作>&"' })
  assert.equal((await call('team.state')).rooms[0].sidebarGroup, null)
  await call('sidebar.move', { conversation: key, group: '研发' })
  await assert.rejects(call('sidebar.move', { conversation: chief.conversation, group: '不存在' }))
  console.log('ok   folders create, rename, collapse, validate and move Bots/group chats without changing models')

  await call('chat.send', { conversation: key, text: 'initial group response' })
  await until('group response', async () => (await items(key)).some(i => i.kind === 'bot'))
  await idle()
  const localPath = join(home, 'team/teambot.json')
  let local = JSON.parse(readFileSync(localPath))
  const savedSeen = local.seen[`${room.id}:${bot.id}`]
  await call('chat.send', { conversation: key, text: 'FAIL_ONCE failed-context\nCONTEXT_EVIDENCE retained-request' })
  await until('failed response', async () => (await items(key)).some(i => i.error))
  await idle()
  local = JSON.parse(readFileSync(localPath))
  assert.equal(local.seen[`${room.id}:${bot.id}`], savedSeen)
  await call('chat.send', { conversation: key, text: 'REPORT_GROUP_CONTEXT retained-request' })
  await until('failed context retained', async () => (await items(key)).some(i => i.kind === 'bot' && i.text === 'Group context retained-request: true'))
  await idle()
  console.log('ok   failed group replies retain unprocessed request context for the next turn')

  await app.stop()
  local = JSON.parse(readFileSync(localPath)); local.sessions[seat] = 'unsupported-session'
  writeFileSync(localPath, JSON.stringify(local))
  app = await startServer({ directory: home, keepHome: true })
  let start = runs().length
  await app.call('chat.send', { conversation: key, text: 'recover unsupported resumed model' })
  await until('model recovered', async () => (await items(key)).some(i => i.kind === 'bot' && i.text.includes('recover unsupported resumed model')))
  await idle()
  let attempts = runs().slice(start)
  // A session the engine cannot resume never reaches the model: one fresh attempt.
  assert.equal(attempts.length, 1)
  assert.ok(attempts[0].includes('--session-id'))
  assert.ok(attempts.every(a => a[a.indexOf('--model') + 1] === 'model-b'))
  console.log('ok   unsupported retained session restarts once using the same Bot model')

  await app.stop()
  local = JSON.parse(readFileSync(localPath))
  // serde uses camelCase; seed a legacy seat with no creation configuration.
  delete local.sessionConfigs; local.sessions[seat] = 'unsupported-session'
  writeFileSync(localPath, JSON.stringify(local))
  app = await startServer({ directory: home, keepHome: true })
  start = runs().length
  const priorReplies = (await items(key)).filter(i => i.kind === 'bot').length
  await app.call('chat.send', { conversation: key, text: 'legacy model configuration recovered' })
  await until('legacy recovered', async () => (await items(key)).filter(i => i.kind === 'bot').length > priorReplies)
  await idle()
  attempts = runs().slice(start)
  assert.equal(attempts.length, 1)
  assert.ok(attempts[0].includes('--session-id'))
  state = await app.call('team.state')
  assert.deepEqual(state.sidebarGroups, ['研发'])
  assert.deepEqual(state.prefs.sidebarCollapsed, ['研发'])
  assert.equal(state.rooms[0].sidebarGroup, '研发')
  await app.call('sidebar.group.delete', { name: '研发' })
  state = await app.call('team.state')
  assert.deepEqual(state.sidebarGroups, [])
  assert.deepEqual(state.prefs.sidebarCollapsed, [])
  assert.equal(state.rooms[0].sidebarGroup, null)
  assert.equal(state.bots.find(b => b.id === bot.id).sidebarGroup, undefined)
  console.log('ok   legacy sessions rebuild safely; folders/collapse survive restart; deleting keeps all conversations')
} finally {
  await app?.stop()
  if (home) rmSync(home, { recursive: true, force: true })
}
