#!/usr/bin/env node
// Hermes-style learning: no separate review or retrieval model calls by default; Bots
// patch their own skills in the turn and search past conversations on demand.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { startServer, until } from './harness.mjs'
let app
try {
  app = await startServer({ extraEnv: { DOTBOT_LEARNING_DELAY_MS: '300', DOTBOT_SKILL_DEFAULTS: '0' } })
  const { call } = app
  const state = () => call('team.state'), items = conversation => call('chat.page', { conversation }).then(x => x.items)
  const chief = (await state()).bots[0], chat = chief.conversation
  const idle = () => until('idle', async () => !(await state()).work.runs[chat])
  const schemaCalls = name => readFileSync(app.log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args)
    .filter(args => args.includes('--json-schema') && JSON.parse(args[args.indexOf('--json-schema') + 1]).properties[name])

  assert.notEqual((await state()).prefs.learningEnabled, true, 'background reviews are opt-in')
  await call('chat.send', { conversation: chat, text: '分析并整理 AUTO_LEARN_SUCCESS inline-no-review\nTOOL task_checkpoint {"summary":"checked"}' })
  await idle()
  await new Promise(resolve => setTimeout(resolve, 2000))
  assert.equal(schemaCalls('reflection').length, 0, 'no separate per-task review call by default')
  assert.equal(schemaCalls('experienceSelection').length, 0, 'no model call to pick past lessons')
  assert.equal((await call('learning.list', { bot: chief.id })).length, 0)
  console.log('ok   finished tasks trigger no background review and replies make no lesson-selection model call')

  // A workflow the Bot wrote improves in place, revision-checked; an imported or user
  // skill still goes through a staged candidate.
  await call('chat.send', { conversation: chat, text: `TOOL save_skill ${JSON.stringify({ name: 'inline-report', description: '整理周报', steps: '## 步骤\n收集本周事项，按项目分组，核对数字后输出周报。' })}` })
  await idle()
  const authored = await call('skill.get', { name: 'inline-report' })
  assert.equal(authored.origin.kind, 'authored')
  await call('chat.send', { conversation: chat, text: `TOOL save_skill ${JSON.stringify({ name: 'inline-report', description: '整理周报', steps: '## 步骤\n收集本周事项，按项目分组，核对数字与上周差异后输出周报。', revision: authored.revision })}` })
  await idle()
  const patched = await call('skill.get', { name: 'inline-report' })
  assert.match(patched.body, /与上周差异/, 'a Bot-authored skill is patched in place')
  assert.equal(patched.origin.kind, 'authored'); assert.equal(patched.origin.updatedBy, chief.id)
  await call('chat.send', { conversation: chat, text: `TOOL save_skill ${JSON.stringify({ name: 'inline-report', description: '整理周报', steps: '过期版本', revision: authored.revision })}` })
  await idle()
  assert.match((await call('skill.get', { name: 'inline-report' })).body, /与上周差异/, 'a stale revision cannot overwrite a newer patch')
  await call('skill.save', { name: 'user-written', description: '用户写的流程', text: '## 步骤\n用户亲自编写的流程，修改前需要验证。' })
  const user = await call('skill.get', { name: 'user-written' })
  await call('chat.send', { conversation: chat, text: `TOOL save_skill ${JSON.stringify({ name: 'user-written', description: '用户写的流程', steps: 'Bot 改写的版本', revision: user.revision })}` })
  await idle()
  assert.match((await call('skill.get', { name: 'user-written' })).body, /用户亲自编写/, 'user skills keep the staged candidate path')
  assert.ok((await items(chat)).some(i => i.kind === 'bot' && i.text.includes('已保存技能候选')))
  console.log('ok   Bot-authored skills improve in place with revision checks; user skills still stage candidates')

  // Session search covers the Bot's own chats and groups it belongs to, never others.
  const other = await call('bot.create', { name: 'Other', role: 'Writer' })
  const room = await call('room.create', { name: '周报群', members: [chief.id], admin: chief.id })
  await call('chat.send', { conversation: `room:${room.id}`, text: '群里约定 alpha-recall-group 用表格格式' })
  await until('group idle', async () => !Object.keys((await state()).work.runs).some(k => k.startsWith(`room:${room.id}`)))
  await call('chat.send', { conversation: `bot:${other.id}`, text: 'alpha-recall-private-other 不应被 Chief 搜到' })
  await until('other idle', async () => !(await state()).work.runs[`bot:${other.id}`])
  await call('chat.send', { conversation: chat, text: '记住 alpha-recall-own 的截止日期是周五' })
  await idle()
  await call('chat.send', { conversation: chat, text: `TOOL search_history ${JSON.stringify({ query: 'alpha-recall' })}` })
  await idle()
  const found = (await items(chat)).findLast(i => i.kind === 'bot' && i.text.startsWith('Done:')).text
  assert.match(found, /alpha-recall-own/); assert.match(found, /alpha-recall-group/); assert.match(found, /group 周报群/)
  assert.doesNotMatch(found, /alpha-recall-private-other/, 'other Bots\' private chats are not searchable')
  await call('bot.update', { id: chief.id, permission: 'readonly' })
  await call('chat.send', { conversation: chat, text: `TOOL search_history ${JSON.stringify({ query: 'alpha-recall-own 周五' })}` })
  await idle()
  assert.match((await items(chat)).findLast(i => i.kind === 'bot').text, /"total":\d+/, 'read-only Bots can search history')
  console.log('ok   search_history finds the Bot\'s own and group conversations only, and works for read-only Bots')
} finally {
  await app?.stop()
}
