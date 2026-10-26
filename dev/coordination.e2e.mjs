#!/usr/bin/env node
// Real scheduler/delegation with a deterministic tool-free planner; no paid models.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { startServer, until } from './harness.mjs'

const app = await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined})
const { call, log } = app
const items = async conversation => (await call('chat.page', { conversation })).items
const tasks = () => call('task.list')
const idle = conversation => until('conversation idle', async () => !(await call('team.state')).work.runs[conversation])
const logs = () => readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line).args)
const plans = () => logs().filter(args => args.includes('--json-schema') && JSON.parse(args[args.indexOf('--json-schema') + 1]).properties.assignments)
try {
  const chief = (await call('team.state')).bots[0]
  await call('bot.update', { id: chief.id, model: 'coordinator-model' })
  const reviewer = await call('bot.create', { name: 'Reviewer', role: 'Review sources and verify evidence', model: 'specialist-model' })
  const writer = await call('bot.create', { name: 'Writer', role: 'Write copy', model: 'writer-model' })
  const verify = async (conversation, token) => {
    const prior = new Set((await tasks()).map(t => t.id)), start = (await items(conversation)).length
    await call('chat.send', { conversation, text: `请核查资料并整理结论 AUTO_COORDINATE ${token}` })
    const task = await until('automatic assignment', async () => (await tasks()).find(t => !prior.has(t.id) && t.automatic && t.text.includes(token)))
    assert.equal(task.target, reviewer.id)
    assert.ok(task.request)
    assert.match(task.text, /Automatically assigned task/)
    assert.ok(task.text.includes(`预期交付：Evidence for ${token}`) && task.text.includes('核对标准：Include verified findings'))
    await until('integrated answer', async () => (await items(conversation)).slice(start).some(i => i.kind === 'bot' && i.text === `Integrated: ${token}`))
    assert.equal((await tasks()).find(t => t.id === task.id).status, 'completed')
    assert.equal((await tasks()).filter(t => t.request === task.request).length, 1)
    return task
  }
  const direct = await verify(chief.conversation, 'main_case')
  assert.equal(direct.targetConversation.split('~task~')[0], `bot:${reviewer.id}`)
  assert.ok((await items(`bot:${reviewer.id}`)).some(i => i.kind === 'incoming' && i.text.includes('[Automatically assigned task]')))
  assert.equal(plans()[0][plans()[0].indexOf('--model') + 1], 'fast-model', 'allocation is pre-reply planning and runs on the planning tier')
  const workerRuns = logs().filter(args => args.includes('--system-prompt') && args[args.indexOf('--system-prompt') + 1].includes('你是 Reviewer，'))
  assert.ok(workerRuns.length)
  assert.ok(workerRuns.every(args => args[args.indexOf('--model') + 1] === 'specialist-model'))
  console.log('ok   Main Bot automatically selects a specialist on the planning tier, supplies bounded tasks, receives results and preserves worker models')

  const structuredInputs = () => readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map(line => JSON.parse(line)).filter(entry => entry.structuredInput && entry.args.includes('--json-schema') && JSON.parse(entry.args[entry.args.indexOf('--json-schema') + 1]).properties.assignments).map(entry => JSON.parse(entry.structuredInput))
  const longStart = structuredInputs().length
  const longRequest = `请核查资料并整理结论 AUTO_COORDINATE long_request\n${'背景资料。'.repeat(4500)}\n末尾硬约束：不得发布；最终只交中文结论 LONG_REQUEST_TAIL`
  await call('chat.send', { conversation: chief.conversation, text: longRequest })
  await until('full long request reaches planner', async () => structuredInputs().slice(longStart).some(input => input.request.includes('LONG_REQUEST_TAIL')))
  const longPlan = structuredInputs().slice(longStart).find(input => input.request.includes('LONG_REQUEST_TAIL'))
  assert.ok(longPlan.request.includes(longRequest), `planner must retain the complete user request beyond character 20000 (received ${longPlan.request.length} characters)`)
  await idle(chief.conversation)
  console.log('ok   allocation receives the tail of requests longer than 20000 characters')

  const room = await call('room.create', { name: 'Automatic group', members: [chief.id, reviewer.id, writer.id], admin: chief.id })
  const conversation = `room:${room.id}`
  assert.equal(room.mode, 'auto')
  const group = await verify(conversation, 'group_case')
  assert.equal(group.source, `${conversation}:${chief.id}`)
  assert.equal(group.targetConversation.split('~task~')[0], `${conversation}:${reviewer.id}`)
  const groupItems = await items(conversation)
  assert.ok(groupItems.some(i => i.who === 'Reviewer' && i.text === 'AUTO_RESULT group_case verified'))
  assert.ok(groupItems.some(i => i.who === 'Chief' && i.text === 'Integrated: group_case'))
  assert.equal((await items(`bot:${reviewer.id}`)).some(i => i.text?.includes('group_case')), false)
  assert.equal((await items(chief.conversation)).some(i => i.text?.includes('group_case')), false)
  console.log('ok   group allocation executes in member seats, exposes results/synthesis in the group and keeps private chats separate')

  await until('group idle before claims', async () => !Object.keys((await call('team.state')).work.runs).some(k => k.startsWith(conversation)))
  const claimPlans = plans().length, claimTasks = (await tasks()).length, claimStart = (await items(conversation)).length
  await call('chat.send', { conversation, text: '核查资料并写一段文案 CLAIM_Reviewer check_part CLAIM_Writer copy_part' })
  await until('claimed parts answered', async () => { const said = (await items(conversation)).slice(claimStart); return said.some(i => i.who === 'Reviewer' && i.text === 'Claimed work: check_part') && said.some(i => i.who === 'Writer' && i.text === 'Claimed work: copy_part') })
  await until('claim round idle', async () => !Object.keys((await call('team.state')).work.runs).some(k => k.startsWith(conversation)))
  assert.equal((await items(conversation)).slice(claimStart).some(i => i.kind === 'bot' && i.who === 'Chief'), false, 'a member that claims nothing stays silent')
  assert.equal(plans().length, claimPlans, 'claimed messages are not allocated again')
  assert.equal((await tasks()).length, claimTasks)
  console.log('ok   un-addressed group messages are answered only by the members that claim a part, without allocation')

  const before = (await tasks()).length
  await call('chat.send', { conversation, text: '@Writer AUTO_COORDINATE explicit_member' })
  await until('explicit member answer', async () => (await items(conversation)).some(i => i.who === 'Writer' && i.text.includes('explicit_member')))
  assert.equal((await tasks()).length, before)
  for (const mode of ['all', 'admin']) {
    await call('room.update', { id: room.id, mode })
    const count = plans().length
    await call('chat.send', { conversation, text: `AUTO_COORDINATE mode_${mode}` })
    await until(`manual mode ${mode} answer`, async () => (await items(conversation)).some(i => i.kind === 'bot' && i.text.includes(`mode_${mode}`))).catch(async error => { console.error(JSON.stringify((await items(conversation)).slice(-10))); throw error })
    // Every member seat settles, so a reply to this round cannot land after the next message.
    await until('group round idle', async () => !Object.keys((await call('team.state')).work.runs).some(k => k.startsWith(`${conversation}:`)))
    assert.equal(plans().length, count)
    assert.equal((await tasks()).length, before)
  }
  await call('room.update', { id: room.id, mode: 'auto' })
  for (const text of ['你好', 'work alone AUTO_COORDINATE alone_case', 'AUTO_COORDINATE_INVALID invalid_case']) {
    await call('chat.send', { conversation: chief.conversation, text })
    await idle(chief.conversation)
    assert.equal((await tasks()).length, before)
  }
  console.log('ok   simple/work-alone requests, explicit addressing, all/admin modes and invalid plans do not start unwanted tasks')

  // No speculative planner call before a normal reply to a pure social turn.
  for (const text of ['你好！', '谢谢你。', 'Hello!']) {
    await idle(chief.conversation)
    const count = plans().length, start = (await items(chief.conversation)).length
    await call('chat.send', { conversation: chief.conversation, text })
    await until('social reply', async () => (await items(chief.conversation)).slice(start).some(i => i.kind === 'bot'))
    await idle(chief.conversation)
    assert.equal(plans().length, count, 'pure social messages bypass automatic allocation')
    assert.equal((await tasks()).length, before)
  }
  console.log('ok   pure greetings and thanks receive a normal reply without an allocation model call')

  await until('group idle before social turn', async () => !Object.keys((await call('team.state')).work.runs).some(k => k.startsWith(conversation)))
  const groupSocialStart = (await items(conversation)).length
  const preparationCalls = () => logs().filter(args => {
    if (!args.includes('--json-schema')) return false
    const properties = JSON.parse(args[args.indexOf('--json-schema') + 1]).properties
    return properties.claim || properties.assignments || properties.skillPlan
  }).length
  const priorPreparation = preparationCalls()
  await call('chat.send', { conversation, text: '你好！' })
  await until('group social answer', async () => (await items(conversation)).slice(groupSocialStart).some(i => i.kind === 'bot' && i.who === 'Chief'))
  await until('group social round idle', async () => !Object.keys((await call('team.state')).work.runs).some(k => k.startsWith(conversation)))
  assert.equal(preparationCalls(), priorPreparation, 'group greeting bypasses member claims, allocation and skill preparation')
  assert.equal((await items(conversation)).slice(groupSocialStart).filter(i => i.kind === 'bot').length, 1, 'only the group admin responds')
  console.log('ok   a group greeting gets one admin reply without member claims, allocation or skill preparation')

  await call('chat.send', { conversation, text: 'AUTO_COORDINATE_FAIL failed_case' })
  const failed = await until('failed specialist', async () => (await tasks()).find(t => t.text.includes('failed_case') && t.status === 'failed'))
  assert.match(failed.result, /allocated-failure/)
  await until('failed result returns', async () => (await items(conversation)).some(i => i.kind === 'bot' && /Task failed: .*allocated-failure/.test(i.text)))
  console.log('ok   failed assignments retain failure state and return to the group coordinator')

  await idle(chief.conversation)
  const planCount = plans().length
  await call('chat.send', { conversation: chief.conversation, text: 'AUTO_COORDINATE_SLOW stopped_planning' })
  await until('planner active', async () => (await call('team.state')).work.runs[chief.conversation]?.activity === 'allocating' && plans().length > planCount)
  await call('chat.stop', { conversation: chief.conversation })
  await idle(chief.conversation)
  assert.equal((await tasks()).some(t => t.text.includes('stopped_planning')), false)

  await call('chat.send', { conversation, text: 'AUTO_COORDINATE stopped_group HOLD_WORK' })
  const stopped = await until('group member running', async () => (await tasks()).find(t => t.text.includes('stopped_group') && t.status === 'running'))
  await call('chat.stop', { conversation })
  await until('all group seats stopped', async () => !Object.keys((await call('team.state')).work.runs).some(key => key.startsWith(`${conversation}:`)))
  assert.equal((await tasks()).find(t => t.id === stopped.id).status, 'cancelled')
  assert.equal((await items(conversation)).some(i => i.text === 'Integrated: stopped_group'), false)
  const again = await verify(conversation, 'group_case')
  assert.notEqual(again.request, group.request)
  console.log('ok   Stop cancels planning and group assignments, suppresses late synthesis and a later request can allocate again')

  const uploaded = await fetch(`${app.base}/api/files?${new URLSearchParams({ conversation, name: 'evidence.txt' })}`, { method: 'POST', headers: { cookie: app.cookie, 'content-type': 'application/octet-stream' }, body: 'shared-evidence-marker' })
  const file = (await uploaded.json()).file
  await call('chat.send', { conversation, text: 'AUTO_COORDINATE attachment_case SHARE_FILE', files: [file.src] })
  const attachmentTask = await until('assigned file reaches member', async () => (await tasks()).find(t => t.text.includes('attachment_case') && t.status === 'completed'))
  assert.deepEqual(attachmentTask.inputs, [file.src])
  assert.match(attachmentTask.result, /attachment=true/)
  assert.equal((await items(conversation)).filter(i => i.kind === 'file' && i.src === file.src).length, 0, 'already-visible input is not reposted as a second file card')
  assert.equal((await items(`bot:${reviewer.id}`)).some(i => i.src === file.src || i.files?.some(f => f.src === file.src)), false)
  await until('attachment result integrated', async () => (await items(conversation)).some(i => i.text === 'Integrated: attachment_case'))

  await until('group idle before inherited attachment', async () => !Object.keys((await call('team.state')).work.runs).some(key=>key.startsWith(`${conversation}:`)))
  await call('chat.send', {conversation, text:'AUTO_COORDINATE attachment_inherited SHARE_FILE INHERIT_FILES', files:[file.src]})
  const inherited = await until('automatic planner omission inherits file', async () => (await tasks()).find(t=>t.text.includes('attachment_inherited')&&t.status==='completed'))
  assert.deepEqual(inherited.inputs,[file.src])
  assert.match(inherited.result,/attachment=true/)
  await until('inherited attachment result integrated', async () => (await items(conversation)).some(i=>i.text==='Integrated: attachment_inherited'))

  const routine = await call('routine.create' , { title: 'Automatic recurring collaboration', target: chief.conversation, prompt: 'AUTO_COORDINATE recurring_case', when: { kind: 'webhook' } })
  await call('routine.run', { id: routine.id })
  await until('recurring work allocated', async () => (await tasks()).some(t => t.automatic && t.text.includes('recurring_case') && t.status === 'completed'))
  await until('recurring result integrated', async () => (await items(chief.conversation)).some(i => i.text === 'Integrated: recurring_case'))
  console.log('ok   group attachment inputs are shared without duplicate cards or private leakage; Main Bot routines can also allocate work')
} finally {
  await app.stop()
}
