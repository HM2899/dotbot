#!/usr/bin/env node
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { execFileSync } from 'node:child_process'
import { readFileSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until, root } from './harness.mjs'

let app, server, browser, context, page, owned = []
const descendants = pid => {
  const rows = execFileSync('ps', ['-eo', 'pid=,ppid=']).toString().trim().split('\n').map(x => x.trim().split(/\s+/).map(Number)), set = new Set([pid]); let count
  do { count = set.size; for (const [id, parent] of rows) if (set.has(parent)) set.add(id) } while (count !== set.size)
  return [...set]
}
const running = pid => { try { return readFileSync(`/proc/${pid}/stat`, 'utf8').split(') ')[1][0] !== 'Z' } catch { return false } }
try {
  app = await startServer()
  await app.call('settings.lang', { lang: 'zh' })
  const bot = await app.call('bot.create', { name: 'bot1', role: '开发', model: 'model-b' })
  const room = await app.call('room.create', { name: '产品讨论', members: [bot.id] })
  const botKey = `bot:${bot.id}`, roomKey = `room:${room.id}`
  server = await chromium.launchServer({ headless: true, executablePath: process.env.DOTBOT_CHROME || undefined })
  owned = descendants(server.process().pid)
  browser = await chromium.connect(server.wsEndpoint())
  context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
  page = await context.newPage()
  const errors = []; page.on('pageerror', e => errors.push(e.message)); page.on('dialog', d => d.accept())
  await app.authorize(context); await page.goto(app.address)
  await page.locator('#bots .row').first().waitFor()
  await page.locator('#new-conversation > summary').click()
  assert.equal(await page.locator('#new-sidebar-group').textContent(), '新建分组')
  await page.locator('#new-sidebar-group').click()
  await page.locator('#sg-name').fill('研发 <A>')
  await page.locator(`.sidebar-group-members input[value="${botKey}"]`).check()
  await page.locator(`.sidebar-group-members input[value="${roomKey}"]`).check()
  await page.locator('#sg-save').click()
  await page.locator('#modal').waitFor({ state: 'hidden' })
  const toggle = page.locator('[data-group-toggle="研发 <A>"]')
  await toggle.waitFor()
  assert.equal(await toggle.locator('small').textContent(), '2')
  await toggle.click()
  await until('collapsed', async () => await toggle.getAttribute('aria-expanded') === 'false')
  assert.equal(await page.locator(`#bots [data-open="${botKey}"]`).count(), 0)
  await page.reload(); await toggle.waitFor()
  assert.equal(await toggle.getAttribute('aria-expanded'), 'false')
  await page.locator('#team-search').fill('产品讨论')
  await page.locator(`#bots [data-open="${roomKey}"]`).waitFor()
  await page.locator('#team-search').fill('')
  await until('collapsed after search', async () => await page.locator(`#bots [data-open="${roomKey}"]`).count() === 0)
  await page.locator('[data-group-edit="研发 <A>"]').click()
  await page.locator('#sg-name').fill('产品研发')
  await page.locator('#sg-save').click()
  await page.locator('#modal').waitFor({ state: 'hidden' })
  await page.locator('[data-group-toggle="产品研发"]').click()
  await page.locator(`#bots [data-open="${roomKey}"]`).waitFor()
  const artifacts = join(root, 'test-results'); mkdirSync(artifacts, { recursive: true })
  for (const theme of ['light', 'dark']) {
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme)
    await page.locator('#new-conversation > summary').hover()
    const geometry = await page.locator('#new-conversation > summary').evaluate(el => {
      const a = el.getBoundingClientRect(), b = el.querySelector('svg').getBoundingClientRect(), s = getComputedStyle(el)
      return { width: a.width, height: a.height, offsetX: (b.x + b.width / 2) - (a.x + a.width / 2), offsetY: (b.y + b.height / 2) - (a.y + a.height / 2), shadow: s.boxShadow, radius: s.borderRadius }
    })
    assert.deepEqual(geometry, { width: 30, height: 30, offsetX: 0, offsetY: 0, shadow: 'none', radius: '8px' })
    await page.screenshot({ path: join(artifacts, `conversation-groups-${theme}.png`), fullPage: true, animations: 'disabled' })
  }
  await page.setViewportSize({ width: 390, height: 844 })
  await page.locator('#toggle-sidebar').click()
  await page.locator('#new-conversation > summary').click()
  await page.locator('#new-sidebar-group').waitFor({ state: 'visible' })
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
  await page.screenshot({ path: join(artifacts, 'conversation-groups-mobile.png'), fullPage: true, animations: 'disabled' })
  await page.locator('#new-conversation > summary').click()
  await page.locator('[data-group-edit="产品研发"]').click()
  await page.locator('#sg-delete').click()
  await page.locator('#modal').waitFor({ state: 'hidden' })
  await until('group removed', async () => !(await app.call('team.state')).sidebarGroups.length)
  await page.locator(`#bots [data-open="${roomKey}"]`).waitFor()
  assert.deepEqual(errors, [])
  console.log('ok   Chinese folder create/edit/delete, collapse persistence, search, desktop/mobile and centered + hover in both themes')
} finally {
  if (server && running(server.process().pid)) owned = descendants(server.process().pid)
  await page?.close().catch(() => {}); await context?.close().catch(() => {}); await browser?.close().catch(() => {}); await server?.close().catch(() => {})
  for (const pid of owned.filter(running)) { try { process.kill(pid, 'SIGTERM') } catch {} }
  let remaining = []
  try { await until('owned browser processes closed', () => owned.every(pid => !running(pid)), 5000) } catch { remaining = owned.filter(running) }
  await app?.stop()
  assert.deepEqual(remaining, [], `Browser cleanup failed for PIDs: ${remaining.join(', ')}`)
  console.log('ok   task-owned browser and test service closed')
}
