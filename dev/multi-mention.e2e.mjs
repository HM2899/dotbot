#!/usr/bin/env node
// Exercise routing and round identity with deterministic model replies, no browser.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { startServer, until } from './harness.mjs'

let app
try {
  app = await startServer({ extraEnv: { DOTBOT_SKILL_DEFAULTS: '0' } })
  const { call } = app
  await call('settings.learning', { enabled: false })
  const chief = (await call('team.state')).bots[0]
  const alpha = await call('bot.create', { name: 'Alpha', role: 'Writer' })
  const beta = await call('bot.create', { name: 'Beta', role: 'Reviewer' })
  const room = await call('room.create', { name: 'Multi mentions', members: [chief.id, alpha.id, beta.id], admin: chief.id })
  const conversation = `room:${room.id}`
  const records = async (key = conversation) => (await call('chat.page', { conversation: key })).items
  const results = async (key = conversation) => (await records(key)).filter(i => i.kind === 'bot' && i.text?.startsWith('MENTION_RESULT ')).map(i => JSON.parse(i.text.split('\n')[0].slice('MENTION_RESULT '.length)))
  const idle = key => until('whole group round idle', async () => !Object.keys((await call('team.state')).work.runs).some(k => k === key || k.startsWith(`${key}:`)))

  await call('chat.send', { conversation, text: '@Beta 核查，委派 @Alpha 按核查结果写作。MULTI_MENTION_CASE ordered' })
  await idle(conversation)
  let rows = (await results()).filter(r => r.token === 'ordered')
  assert.deepEqual(rows.map(r => r.bot), [beta.id, alpha.id], 'all named recipients run in textual order, even with explicit delegation wording')
  assert.deepEqual(rows[0].pending, ['Alpha'])
  assert.equal(rows[0].prior, 0)
  assert.equal(rows[1].prior, 1)
  assert.deepEqual(rows[1].pending, [])
  assert.equal(rows[0].request, rows[1].request)
  assert.equal((await call('task.list')).length, 0, 'named recipients must not also trigger generic delegation')
  console.log('ok   multiple mentions override generic delegation, preserve order and expose pending members')

  await call('chat.send', { conversation, text: '委派 @all @Alpha 各完成自己的部分。MULTI_MENTION_CASE everyone' })
  await idle(conversation)
  rows = (await results()).filter(r => r.token === 'everyone')
  assert.deepEqual(rows.map(r => r.bot), [chief.id, alpha.id, beta.id], '@all retains every member even alongside a named mention and delegation wording')
  assert.equal((await call('task.list')).length, 0)
  console.log('ok   @all is not reduced to a single named recipient or generic delegate')

  await call('chat.send', { conversation, text: '@Alpha 做第一项，@Beta 做第二项。MULTI_MENTION_CASE overlap_a\nDELAY 500' })
  await call('chat.send', { conversation, text: '@Beta 做另一项，@Alpha 补另一份结果。MULTI_MENTION_CASE overlap_b' })
  await idle(conversation)
  for (const [token, expected] of [['overlap_a', [alpha.id, beta.id]], ['overlap_b', [beta.id, alpha.id]]]) {
    rows = (await results()).filter(r => r.token === token)
    assert.deepEqual(rows.map(r => r.bot), expected, 'overlapping rounds must retain their own request after read cursors advance')
    assert.equal(new Set(rows.map(r => r.request)).size, 1)
  }
  assert.equal(new Set((await results()).filter(r => r.token.startsWith('overlap_')).map(r => r.request)).size, 2)
  console.log('ok   overlapping multi-mention rounds retain distinct identities and do not drop either request')

  await call('chat.send', { conversation, text: '@Alpha 开始制作，再请其他成员补充。MULTI_MENTION_CASE followup' })
  await idle(conversation)
  rows = (await results()).filter(r => r.token === 'followup')
  assert.deepEqual(rows.map(r => r.bot), [alpha.id, beta.id, alpha.id], 'an explicit handoff can revisit an earlier member for real dependent work')
  console.log('ok   explicit follow-up handoffs still support produce, review and revise')

  await call('chat.send', { conversation, text: '@Alpha 开始。MULTI_MENTION_CASE loop' })
  await idle(conversation)
  const limited = (await call('recovery.list')).find(w => w.conversation === conversation && w.text.includes('MULTI_MENTION_CASE loop'))
  assert.equal(limited?.status, 'failed', 'a round ending with pending handoffs must not be marked completed')
  assert.match(limited.result.error, /上限/)
  console.log('ok   the turn limit records unresolved handoffs as failure instead of silent completion')

  const before = new Set((await call('task.list')).map(t => t.id))
  const sends = [
    {bot: alpha.name, text: 'DELAY 2500\nslow-out-of-order', reply_to: 'me'},
    {bot: beta.name, text: 'fast-out-of-order', reply_to: 'me'},
  ]
  await call('chat.send', { conversation: chief.conversation, text: sends.map(args => `TOOL message_bot ${JSON.stringify(args)}`).join('\n') })
  await until('both private delegated results return', async () => {
    const tasks = (await call('task.list')).filter(t => !before.has(t.id))
    const incoming = (await records(chief.conversation)).filter(i => i.kind === 'incoming' && i.text?.startsWith('[Reply from '))
    return tasks.length === 2 && tasks.every(t => t.status === 'completed') && incoming.length >= 2
  })
  await idle(chief.conversation)
  const snapshots = readFileSync(app.log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args).filter(args => args.includes('--system-prompt')).map(args => args[args.indexOf('--system-prompt') + 1]).map(system => system.match(/# 同一请求的协作状态\n[^\n]+\n(\[[^\n]*\])/)).filter(Boolean).map(match => JSON.parse(match[1]))
  assert.ok(snapshots.some(tasks => tasks.some(t => t.target === beta.id && t.status === 'completed') && tasks.some(t => t.target === alpha.id && ['queued', 'running'].includes(t.status))), 'fast replies must expose the still-pending sibling, not imply whole-request completion')
  console.log('ok   out-of-order private replies include the other task’s actual pending state')

  const beforeCreate = new Set((await call('task.list')).map(t => t.id))
  await call('chat.send', { conversation: chief.conversation, text: [
    `TOOL create_bot ${JSON.stringify({name:'CreatedWorker',role:'Reviewer',instructions:'完成分配部分并回传实际结果。',task:'DELAY 1000\ncreated-worker-result'})}`,
    `TOOL message_bot ${JSON.stringify({bot:beta.name,text:'after-create-result',reply_to:'me'})}`,
  ].join('\n') })
  await until('creation handoff does not drop next recipient', async () => {
    const tasks = (await call('task.list')).filter(t => !beforeCreate.has(t.id))
    return tasks.length === 2 && tasks.every(t => t.status === 'completed')
  })
  await idle(chief.conversation)
  console.log('ok   creating a worker with a task does not cut off the next handoff')

  const many = []
  for (let i = 0; i < 9; i++) many.push(await call('bot.create', { name: `Member${i}`, role: 'Specialist' }))
  const large = await call('room.create', { name: 'More than eight', members: [chief.id, ...many.map(b => b.id)], admin: chief.id })
  const largeKey = `room:${large.id}`
  const reversed = [...many].reverse()
  await call('chat.send', { conversation: largeKey, text: `${reversed.map(b => `@${b.name}`).join(' ')} 各完成自己的部分。MULTI_MENTION_CASE many` })
  await idle(largeKey)
  rows = (await results(largeKey)).filter(r => r.token === 'many')
  assert.deepEqual(rows.map(r => r.bot), reversed.map(b => b.id), 'initial addressed members must not be cut off by the normal eight-turn limit')
  assert.ok(rows.every(r => r.errors === 0))
  assert.equal(rows.at(-1).prior, 8)
  console.log('ok   more than eight addressed members each get a turn without being silently dropped')
} finally {
  if (app) await app.stop()
}
