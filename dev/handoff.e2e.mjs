#!/usr/bin/env node
// An outstanding child must not reserve the coordinator's execution slot.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import { startServer, until, root } from './harness.mjs'

// Exercise the actual frontend status function without starting a browser.
const source = readFileSync(`${root}/web/app.js`, 'utf8')
const activity = source.slice(source.indexOf('const botRuns ='), source.indexOf('function hasDelegatedWork('))
const context = vm.createContext({ view: { work: { runs: {}, tasks: [{ status: 'running', source: 'bot:chief' }] } } })
vm.runInContext(activity, context)
assert.equal(vm.runInContext("botActivity({id:'chief',conversation:'bot:chief'})", context), null)
context.view.work.runs['bot:chief'] = { activity: 'thinking' }
assert.equal(vm.runInContext("botActivity({id:'chief',conversation:'bot:chief'})", context), 'thinking')

let app
try {
  app = await startServer({ extraEnv: { DOTBOT_SKILL_DEFAULTS: '0' } })
  const { call } = app
  await call('settings.learning', { enabled: false })
  const chief = (await call('team.state')).bots[0]
  const worker = await call('bot.create', { name: 'HandoffWorker', role: 'Reviewer' })
  const items = () => call('chat.page', { conversation: chief.conversation }).then(p => p.items)
  const idleWithChild = marker => until('coordinator released while child runs', async () => {
    const state = await call('team.state')
    const task = state.work.tasks.find(t => t.source === chief.conversation && t.text.includes(marker) && t.status === 'running')
    return task && !state.work.runs[chief.conversation] && task
  })

  await call('chat.send', { conversation: chief.conversation, text: 'AUTO_COORDINATE handoff_auto HOLD_WORK' })
  const automatic = await idleWithChild('handoff_auto')
  // Dispatch alone must not invoke a normal coordinator model turn.
  const logs = readFileSync(app.log, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line).args)
  assert.ok(!logs.some(args => args.includes('--system-prompt') && args[args.indexOf('--system-prompt') + 1].includes(`你是 ${chief.name}，`)))
  await call('chat.send', { conversation: chief.conversation, text: 'foreground_during_handoff' })
  await until('new user request answered before child finishes', async () => (await items()).some(i => i.kind === 'bot' && i.text?.includes('foreground_during_handoff')))
  assert.equal((await call('task.get', { id: automatic.id })).status, 'running')
  await until('child reply wakes coordinator', async () => (await items()).some(i => i.kind === 'bot' && i.text === 'Integrated: handoff_auto'))

  await until('coordinator idle after integration', async () => !(await call('team.state')).work.runs[chief.conversation])
  await call('chat.send', { conversation: chief.conversation, text: `TOOL message_bot ${JSON.stringify({ bot: worker.name, text: 'DELAY 6000\nmanual_handoff_result' })}` })
  const manual = await idleWithChild('manual_handoff_result')
  await call('chat.send', { conversation: chief.conversation, text: 'foreground_after_manual' })
  await until('manual dispatch also releases coordinator', async () => (await items()).some(i => i.kind === 'bot' && i.text?.includes('foreground_after_manual')))
  assert.equal((await call('task.get', { id: manual.id })).status, 'running')
  await call('task.cancel', { id: manual.id })
  console.log('ok   automatic/manual handoff releases coordinator; user requests run immediately; child replies resume processing; UI stays idle')

  await until('coordinator idle before recovery test', async () => !(await call('team.state')).work.runs[chief.conversation])
  const recoveryStart = (await items()).length
  const failedWork = 'TRANSIENT after-tool checkpoint-return\nTOOL task_checkpoint {"summary":"已核对资料，发送状态仍需确认 checkpoint-return-marker"}'
  await call('chat.send', { conversation: chief.conversation, text: `TOOL message_bot ${JSON.stringify({ bot: worker.name, text: failedWork })}` })
  const failure = await until('failure returns with checkpoint', async () => (await items()).slice(recoveryStart).find(i => i.kind === 'incoming' && i.text?.includes('协作失败前保存的恢复线索')))
  assert.match(failure.text, /Task failed:/)
  assert.match(failure.text, /checkpoint-return-marker/)
  assert.match(failure.text, /"recentActions":/)
  assert.match(failure.text, /未确认的发送、提交等操作先查证，不自动重放/)
  const failedTask = (await call('task.list')).find(t => t.text.includes('checkpoint-return'))
  assert.equal(failedTask.status, 'failed', 'checkpoint context must not turn a failed task into success')
  console.log('ok   a failed collaborator returns its saved checkpoint and action receipts without hiding failure or replaying work')
} finally {
  if (app) await app.stop()
}
