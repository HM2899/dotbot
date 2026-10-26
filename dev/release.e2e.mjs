#!/usr/bin/env node
// Validate the embedded release UI and new APIs without touching the user's service.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, root, until } from './harness.mjs'

let app
try {
  app = await startServer({ binary: join(root, 'target/release/dotbot') })
  for (const file of ['index.html', 'app.js', 'style.css', 'i18n.js']) {
    const response = await fetch(`${app.base}/${file === 'index.html' ? '' : `assets/${file}`}`, { headers: { cookie: app.cookie } })
    assert.equal(response.status, 200)
    assert.equal(await response.text(), readFileSync(join(root, 'web', file), 'utf8'), `embedded ${file} is current`)
  }
  const { call } = app
  const accounts = await call('account.catalog')
  assert.deepEqual(accounts.map(p => p.id), ['google', 'github', 'microsoft'])
  assert.ok(['gmail', 'drive'].every(id => accounts[0].services.some(service => service.id === id)))
  await call('account.save', { provider: 'github', clientId: 'release-smoke', name: 'Release smoke account', scopes: ['read:user'] })
  const state = await call('team.state'), conversation = state.bots[0].conversation
  assert.equal(state.accounts[0].status, 'disconnected')
  assert.deepEqual(await call('delivery.list'), [])
  assert.deepEqual(await call('recovery.list'), [])
  const routine = await call('routine.create', { title: 'Release webhook', target: conversation, prompt: 'fixture only', when: { kind: 'webhook' } })
  assert.equal((await call('event.test', { id: routine.id, payload: { fixture: true } })).local, true)
  await call('event.public-url', { url: 'https://release.example.com' })
  assert.equal((await call('team.state')).accountRedirect, 'https://release.example.com/oauth/callback')
  const setup = await call('event.setup', { id: routine.id })
  assert.ok(setup.webhook.startsWith('https://release.example.com/hook/'))
  assert.ok(!setup.status.localTest, 'changed callback must not reuse old success evidence')
  await call('chat.send', { conversation, text: 'TOOL task_checkpoint {"summary":"release checkpoint verified"}' })
  await until('release execution finished', async () => (await call('recovery.list')).some(w => w.status === 'completed' && w.checkpoint === 'release checkpoint verified'))
  assert.ok((await call('skill.catalog', { query: 'PDF' })).some(s => s.id === 'anthropic/pdf'))
  await until('release Bot idle', async () => !(await call('team.state')).work.runs[conversation])
  await call('chat.send', { conversation, text: '分析资料 AUTO_SKILL_CREATE release' })
  const workflow = await until('release skill created and loaded', async () => (await call('team.state')).bots[0].skillProfile?.find(s => s.name.startsWith('workflow-release') && s.lastOutcome === 'turn-completed'))
  assert.equal((await call('skill.get', { name: workflow.name })).origin.validation, 'untested')
  await call('settings.learning', { enabled: true })
  assert.equal((await call('team.state')).prefs.learningEnabled, true)
  assert.ok(Array.isArray(await call('learning.list', { bot: state.bots[0].id })))
  console.log('ok   release embeds current HTML/JS/CSS/translations; account catalog, public callback, event evidence and durable checkpoint and automatic skill and learning APIs work')
} finally {
  await app?.stop()
  console.log('ok   isolated release service and temporary data cleaned up')
}
