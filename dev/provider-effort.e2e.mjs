#!/usr/bin/env node
// A text provider's default reasoning effort: set on its card or in its edit window, shown in Bot
// details, and sent with a Bot's requests unless the Bot names its own level.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { chromium } from 'playwright'
import { startServer, until } from './harness.mjs'
let app, browser
try {
  app = await startServer({ extraEnv: { DOTBOT_SKILL_DEFAULTS: '0' } })
  const { call } = app
  const provider = (await call('provider.list')).providers.find(p => p.active)
  const requests = name => readFileSync(app.log, 'utf8').trim().split('\n').map(line => JSON.parse(line).args)
    .filter(args => args.includes('--thinking') || args.includes('--reasoning-effort') || args.includes('--system-prompt'))
    .filter(args => args[args.indexOf('--system-prompt') + 1]?.includes(name))
  const ask = async (name, effort) => {
    const bot = await call('bot.create', { name, role: 'Effort check' })
    if (effort !== undefined) await call('bot.update', { id: bot.id, effort })
    bot.conversation = `bot:${bot.id}`
    await call('chat.send', { conversation: bot.conversation, text: `hello ${name}` })
    await until(`${name} answered`, async () => (await call('chat.page', { conversation: bot.conversation })).items.some(i => i.kind === 'bot'))
    const sent = requests(name)[0]
    return { bot, thinking: sent.includes('--thinking') ? JSON.parse(sent[sent.indexOf('--thinking') + 1]) : null }
  }

  // Nothing set anywhere: no reasoning setting is sent.
  assert.equal(provider.defaultEffort ?? '', '')
  assert.equal((await ask('EffortNone1', undefined)).thinking, null)

  browser = await chromium.launch({ executablePath: process.env.DOTBOT_CHROME || undefined })
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, locale: 'zh-CN' })
  await app.authorize(context)
  const page = await context.newPage()
  const errors = []; page.on('pageerror', error => errors.push(error.message))
  await page.goto(`${app.base}/#page:providers`)
  const select = page.locator(`[data-provider-effort="${provider.id}"]`)
  await select.waitFor()
  assert.equal(await select.inputValue(), '')
  assert.match(await select.locator('option').first().textContent(), /不设置/)

  // The card's choice saves at once and reaches the team state the Bot details read.
  await select.selectOption('high')
  await until('default saved', async () => (await call('team.state')).provider.defaultEffort === 'high')
  await page.locator('#toast').filter({ hasText: '默认思考强度已设为：高' }).waitFor()
  assert.equal((await call('provider.list')).providers.find(p => p.id === provider.id).defaultEffort, 'high')

  // A Bot without its own level follows it; its own level, or Off, wins.
  const followed = await ask('EffortFollow', undefined)
  assert.deepEqual(followed.thinking, { type: 'enabled', budget_tokens: 8192 })
  assert.deepEqual((await ask('EffortOwn', 'low')).thinking, { type: 'enabled', budget_tokens: 2048 })
  assert.deepEqual((await ask('EffortOff', 'none')).thinking, { type: 'disabled' })
  await page.evaluate(() => document.querySelector('#settings-close')?.click())
  await page.goto(`${app.base}/`)
  await page.locator(`#bots [data-open="${followed.bot.conversation}"]`).click()
  await page.locator('#conversation-menu > summary').click(); await page.locator('#act-details').click()
  assert.equal((await page.locator('#f-effort option').first().textContent()).trim(), '跟随供应商（高）')
  assert.match(await page.locator('#panel-body .field-hint').filter({ hasText: '默认思考强度' }).textContent(), /留空时跟随/)
  await page.locator('#panel-close, #conversation-menu > summary').first().click().catch(() => {})

  // The edit window carries the same choice with the rest of the provider's settings.
  await page.goto(`${app.base}/#page:providers`); await page.reload()
  await page.locator(`[data-provider-edit="${provider.id}"]`).click()
  assert.equal(await page.locator('#pv-effort').inputValue(), 'high')
  await page.locator('#pv-effort').selectOption('medium')
  await page.locator('#pv-save').click()
  await until('saved from the window', async () => (await call('team.state')).provider.defaultEffort === 'medium')
  assert.equal((await call('provider.list')).providers.find(p => p.id === provider.id).hasKey, true, 'saving the window keeps the key')
  assert.deepEqual((await ask('EffortMedium', undefined)).thinking, { type: 'enabled', budget_tokens: 4096 })

  // Clearing it goes back to sending nothing; an unknown level is refused by the server.
  await page.locator(`[data-provider-effort="${provider.id}"]`).selectOption('')
  await until('cleared', async () => !(await call('team.state')).provider.defaultEffort)
  assert.equal((await ask('EffortNone2', undefined)).thinking, null)
  await assert.rejects(call('provider.effort', { id: provider.id, effort: 'loud' }), /思考强度/)

  // Phones: the card and the edit window fit.
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto(`${app.base}/#page:providers`); await page.reload()
  await page.locator(`[data-provider-effort="${provider.id}"]`).waitFor()
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
  await page.locator(`[data-provider-edit="${provider.id}"]`).click()
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
  assert.deepEqual(errors, [])
  console.log('ok   a provider sets the default reasoning effort from its card or edit window; Bots follow it, override it or turn it off, and clearing it sends nothing')
} finally {
  await browser?.close().catch(() => {})
  await app?.stop()
}
