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
const holds = new Map(), counts = new Map(), failures = new Set()
const hold = key => {
  let release
  const gate = { promise: new Promise(resolve => { release = resolve }), seen: 0, release: () => { holds.delete(key); release() } }
  holds.set(key, gate)
  return gate
}
try {
  app = await startServer()
  await app.call('settings.lang', { lang: 'zh' })
  const chief = (await app.call('team.state')).bots[0]
  const beta = await app.call('bot.create', { name: '加载测试 B', model: 'model-b' })
  const gamma = await app.call('bot.create', { name: '加载测试 C', model: 'model-b' })
  const betaKey = `bot:${beta.id}`, gammaKey = `bot:${gamma.id}`
  for (const [conversation, token] of [[chief.conversation, 'A-history'], [betaKey, 'B-history'], [gammaKey, 'C-history']]) {
    await app.call('chat.send', { conversation, text: `${token}\n${Array.from({length:60}, (_,i)=>`段落 ${i}：保留已阅读内容与当前滚动位置。`).join('\n\n')}` })
    await until('fixture response', async () => (await app.call('chat.page', { conversation })).items.some(i => i.kind === 'bot'))
  }
  await until('idle', async () => !Object.keys((await app.call('team.state')).work.runs).length)
  server = await chromium.launchServer({ headless: true, executablePath: process.env.DOTBOT_CHROME || undefined })
  owned = descendants(server.process().pid)
  browser = await chromium.connect(server.wsEndpoint())
  context = await browser.newContext({ viewport: { width: 1100, height: 850 }, locale: 'zh-CN' })
  page = await context.newPage()
  const errors = []; page.on('pageerror', e => errors.push(e.message))
  await page.route('**/api/bot', async route => {
    const {method, params = {}} = route.request().postDataJSON()
    const key = method === 'chat.page' ? `${method}:${params.conversation}` : method === 'usage.view' ? `${method}:${params.days}` : method
    counts.set(key, (counts.get(key) ?? 0) + 1)
    const gate = holds.get(key)
    if (gate) { gate.seen++; await gate.promise }
    try {
      if (failures.delete(key)) await route.fulfill({status:503, contentType:'application/json', body:JSON.stringify({ok:false,error:{message:'fixture offline'}})})
      else await route.continue()
    } catch {} // Cancelled navigation requests may no longer be fulfilled.
  })
  const boot = hold('team.state'), first = hold(`chat.page:${chief.conversation}`)
  await app.authorize(context); await page.goto(app.address)
  await until('boot request', () => boot.seen)
  await page.locator('#bots .skeleton-sidebar').waitFor()
  await page.locator('#messages .skeleton-chat').waitFor()
  boot.release()
  await until('history request', () => first.seen)
  assert.equal(await page.locator('.chat-welcome').count(), 0)
  assert.equal(await page.locator('#messages').getAttribute('aria-busy'), 'true')
  const artifacts = join(root, 'test-results'); mkdirSync(artifacts, { recursive: true })
  await page.screenshot({ path: join(artifacts, 'loading-chat-desktop.png'), animations: 'disabled' })
  first.release()
  await page.locator('#messages .msg.user').filter({hasText:'A-history'}).waitFor()
  await page.waitForFunction(()=>!view.conversationLoading)
  await page.evaluate(()=>{document.querySelector('#messages').scrollTop=260})
  const beforeScroll = await page.locator('#messages').evaluate(el=>el.scrollTop)
  const betaCold = hold(`chat.page:${betaKey}`)
  await page.locator(`[data-open="${betaKey}"]`).click()
  await until('cold B request', ()=>betaCold.seen)
  await page.locator('#messages .skeleton-chat').waitFor()
  assert.equal(await page.locator('#messages .msg').count(),0)
  betaCold.release()
  await page.locator('#messages .msg.user').filter({hasText:'B-history'}).waitFor()
  await page.waitForFunction(()=>!view.conversationLoading)
  await page.evaluate(()=>{window.fixedMessage=document.querySelector('#messages .msg.bot');window.fixedSidebar=document.querySelector('#bots .row.active')})
  await page.evaluate(({conversation,bot})=>{
    for(let i=0;i<30;i++)onEvent('chat.stream',{conversation,bot,text:'测试片段 '})
  },{conversation:betaKey,bot:beta.id})
  await page.locator('.streaming').waitFor()
  assert.ok(await page.evaluate(()=>fixedMessage===document.querySelector('#messages .msg.bot:not(.streaming)')))
  await page.evaluate(()=>onEvent('chat.meta',view.team.history[view.current]))
  assert.ok(await page.evaluate(()=>fixedSidebar===document.querySelector('#bots .row.active')))
  await page.evaluate(({conversation,bot})=>onEvent('chat.stream',{conversation,bot,phase:'end'}),{conversation:betaKey,bot:beta.id})
  // A changed row before a PDF must not move/reload the unchanged iframe.
  await page.evaluate(async()=>{
    const box=document.createElement('div');document.body.append(box)
    reconcileRows(box,'<div data-id="a">first</div><div data-id="pdf"><iframe src="about:blank"></iframe></div>')
    const frame=box.querySelector('iframe'), frameWindow=frame.contentWindow
    frameWindow.taskMarker='keep-frame-state'
    reconcileRows(box,'<div data-id="a">changed</div><div data-id="pdf"><iframe src="about:blank"></iframe></div>')
    if(frame!==box.querySelector('iframe')||frame.contentWindow!==frameWindow||frame.contentWindow.taskMarker!=='keep-frame-state')throw new Error('unchanged media reloaded')
    box.remove()
  })
  const warm = hold(`chat.page:${chief.conversation}`)
  await page.evaluate(key=>{void open(key)},chief.conversation)
  assert.equal(await page.locator('#messages .loading-skeleton').count(),0)
  assert.ok((await page.locator('#messages').textContent()).includes('A-history'))
  assert.ok(Math.abs((await page.locator('#messages').evaluate(el=>el.scrollTop))-beforeScroll)<5)
  warm.release();await page.waitForFunction(()=>!view.conversationLoading)
  console.log('ok   cold boot/chat skeletons, warm conversation/scroll restoration, stable streaming rows and PDF identity')

  await page.locator('#open-settings').click()
  await page.locator('#p-name').fill('保留未保存姓名')
  const usage = hold('usage.view:30')
  await page.locator('[data-page="usage"]').click()
  await until('usage request',()=>usage.seen)
  await page.locator('#settings-content .skeleton-settings').waitFor()
  assert.equal(await page.locator('#settings-content').getAttribute('aria-busy'),'true')
  const providers = hold('provider.list')
  await page.locator('[data-page="providers"]').click()
  await until('provider request',()=>providers.seen)
  usage.release()
  await page.waitForFunction(()=>Boolean(pageData.usage))
  assert.equal(await page.evaluate(()=>view.page),'providers')
  assert.equal(await page.locator('#settings-content .skeleton-settings').count(),1)
  await page.setViewportSize({width:390,height:844})
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1))
  await page.emulateMedia({reducedMotion:'reduce'})
  assert.equal(await page.locator('.skeleton-block').first().evaluate(el=>getComputedStyle(el,'::after').animationName),'none')
  await page.screenshot({path:join(artifacts,'loading-settings-mobile.png'),animations:'disabled'})
  providers.release();await page.locator('.provider').first().waitFor()
  await page.locator('[data-page="general"]').click()
  assert.equal(await page.locator('#p-name').inputValue(),'保留未保存姓名')
  const countProviders=counts.get('provider.list')
  await page.locator('[data-page="providers"]').click()
  assert.equal(await page.locator('#settings-content .loading-skeleton').count(),0)
  assert.equal(counts.get('provider.list'),countProviders)
  await page.locator('[data-page="usage"]').click()
  assert.equal(await page.locator('#settings-content .loading-skeleton').count(),0)
  const seven=hold('usage.view:7'),ninety=hold('usage.view:90')
  const sevenBefore=counts.get('usage.view:7')??0
  await page.evaluate(()=>{usageView.days=7;void loadPage('usage');void loadPage('usage')})
  await until('7-day request',()=>seven.seen)
  await page.evaluate(()=>{usageView.days=90;void loadPage('usage')})
  await until('90-day request',()=>ninety.seen)
  await page.evaluate(()=>{usageView.days=7;void loadPage('usage')})
  ninety.release()
  await page.waitForFunction(()=>!pageLoads.has('usage:90'))
  assert.equal(await page.evaluate(()=>pageData.usage.days),30)
  seven.release();await page.waitForFunction(()=>pageData.usage.days===7)
  assert.equal((counts.get('usage.view:7')??0)-sevenBefore,1)
  assert.equal(await page.locator('#settings-content').getAttribute('aria-busy'),'false')
  console.log('ok   settings skeletons, mobile/reduced-motion, cross-page response isolation, draft retention, cached views and deduplicated latest-range loads')

  await page.locator('[data-page="skills"]').click();await page.locator('[data-plugin-tab="manual"]').click();await page.locator('#sk-name').waitFor()
  assert.equal(counts.get('skill.catalog')??0,0)
  assert.equal(await page.locator('#settings-content .loading-skeleton, .skill-catalog').count(),0)
  const routines=hold('routine.list');failures.add('routine.list')
  await page.locator('[data-page="routines"]').click();await until('routine request',()=>routines.seen)
  routines.release();await page.locator('#settings-content .load-error').waitFor()
  await page.locator('#settings-content [data-page-retry]').click();await page.locator('[data-routine-new]').waitFor()
  await page.locator('#settings-close').click();await page.locator('#settings').waitFor({state:'hidden'})
  const failed=hold(`chat.page:${gammaKey}`);failures.add(`chat.page:${gammaKey}`)
  await page.evaluate(key=>{void open(key)},gammaKey);await until('failed conversation request',()=>failed.seen)
  failed.release();await page.locator('#messages .load-error').waitFor()
  await page.locator('#messages [data-chat-retry]').click()
  await page.locator('#messages .msg.user').filter({hasText:'C-history'}).waitFor()
  await page.waitForFunction(()=>!view.conversationLoading)
  failures.add(`chat.page:${gammaKey}`)
  await page.evaluate(key=>{void open(key)},gammaKey)
  await page.locator('#chat-retry').waitFor({state:'visible'})
  assert.ok((await page.locator('#messages').textContent()).includes('C-history'))
  await page.locator('#chat-retry').click();await page.waitForFunction(()=>!view.conversationLoading&&!view.conversationError)
  const memory=hold('memory.view')
  await page.evaluate(()=>openPanel('memory'));await until('memory request',()=>memory.seen)
  await page.locator('#panel-body .skeleton-settings').waitFor()
  await page.evaluate(()=>openPanel('details'))
  memory.release();await page.waitForFunction(()=>document.querySelector('#f-name')!==null)
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))))
  assert.equal(await page.locator('#m-ask').count(),0)
  assert.equal(await page.locator('#f-name').inputValue(),gamma.name)
  assert.deepEqual(errors,[])
  console.log('ok   cold/cached load failures keep readable content and retry; stale memory responses cannot replace Bot details')
} finally {
  for (const gate of holds.values()) gate.release()
  if (server && running(server.process().pid)) owned = descendants(server.process().pid)
  await page?.close().catch(() => {}); await context?.close().catch(() => {}); await browser?.close().catch(() => {}); await server?.close().catch(() => {})
  for (const pid of owned.filter(running)) { try { process.kill(pid, 'SIGTERM') } catch {} }
  let remaining = []
  try { await until('owned browser processes closed', () => owned.every(pid => !running(pid)), 5000) } catch { remaining = owned.filter(running) }
  await app?.stop()
  assert.deepEqual(remaining, [], `Browser cleanup failed for PIDs: ${remaining.join(', ')}`)
  console.log('ok   task-owned browser and test service closed')
}
