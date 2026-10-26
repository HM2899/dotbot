#!/usr/bin/env node
// One isolated real desktop container and one browser, context and page. No accounts.
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until, root } from './harness.mjs'
let app, container, browserServer, browser, context, page, display, browserPids = []
const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()
const alive = pid => { try { return readFileSync(`/proc/${pid}/stat`, 'utf8').split(') ')[1][0] !== 'Z' } catch { return false } }
function descendants(pid) {
  const rows = execFileSync('ps', ['-eo', 'pid=,ppid='], { encoding: 'utf8' }).trim().split('\n').map(s => s.trim().split(/\s+/).map(Number))
  const owned = new Set([pid]); let size
  do { size = owned.size; for (const [child, parent] of rows) if (owned.has(parent)) owned.add(child) } while (size !== owned.size)
  return [...owned]
}
try {
  app = await startServer({ desktop: true, browser: true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'} })
  const { call } = app, state = await call('team.state'), bot = state.bots[0]
  assert.equal(state.desktop, true, 'real Docker is required for this suite')
  assert.equal(state.browser, false, 'desktop mode must not start a separate browser')
  assert.equal(state.computerMode, 'desktop')
  container = (await call('desktop.status', { bot: bot.id })).container
  await call('desktop.open', { bot: bot.id })
  await until('desktop ready', async () => (await call('desktop.status', { bot: bot.id })).ready, 180000)
  assert.ok(!readFileSync(join(app.home, 'team/state.json'), 'utf8').includes('playwright'))
  display = (await call('desktop.status', { bot: bot.id })).display
  const home = join(app.home, 'team/desktops/shared')
  writeFileSync(join(home, 'desktop-fixture.html'), '<!doctype html><meta charset="utf-8"><title>桌面浏览器验证</title><style>body{font:24px sans-serif;padding:50px;background:#eef5ff}h1{color:#1768d1}</style><h1>同一桌面，同一浏览器</h1><p>浏览器与软件都在桌面内操作。</p>')
  docker('exec', '-d', '-u', 'abc', '-w', '/config', container, 'python3', '-m', 'http.server', '8844', '--bind', '127.0.0.1')
  const fixtureUrl = 'http://127.0.0.1:8844/desktop-fixture.html'
  await assert.rejects(call('desktop.launch', { bot: bot.id, app: 'browser', url: fixtureUrl }), /Take control/)
  await call('desktop.control', { bot: bot.id, user: true })
  for (const url of ['file:///etc/passwd', 'javascript://example.com/', 'https://user:pass@example.com']) await assert.rejects(call('desktop.launch', { bot: bot.id, app: 'browser', url }), /valid HTTP/)
  await assert.rejects(call('desktop.launch', { bot: bot.id, app: 'shell', url: '' }), /Choose the desktop/)
  const launched = await call('desktop.launch', { bot: bot.id, app: 'browser', url: fixtureUrl })
  assert.equal(launched.surface, 'desktop')
  await until('real desktop browser visible', () => { try { return docker('exec', '-u', 'abc', '-e', `DISPLAY=${display}`, container, 'xdotool', 'search', '--name', '桌面浏览器验证') } catch { return false } }, 60000)
  await call('desktop.control', { bot: bot.id, user: false })
  await call('chat.send', { conversation: bot.conversation, text: 'desktop prompt and transport check' })
  await until('model finished', async () => (await call('chat.page', { conversation: bot.conversation })).items.some(i => i.text === 'Echo: desktop prompt and transport check'))
  const args = readFileSync(app.log, 'utf8').trim().split('\n').map(JSON.parse).map(r => r.args).find(a => a.includes('--mcp-config'))
  assert.equal(JSON.parse(args[args.indexOf('--mcp-config') + 1]).mcpServers.browser, undefined)
  assert.match(args[args.indexOf('--system-prompt') + 1], /浏览器、网站和软件都指这台电脑上的应用/)
  await call('bot.update', { id: bot.id, permission: 'edits' })
  await call('chat.send', { conversation: bot.conversation, text: `TOOL desktop_browser ${JSON.stringify({ url: fixtureUrl })}` })
  const approval = await until('desktop browser approval', async () => (await call('team.state')).work.approvals.find(a => a.tool === 'desktop_browser'))
  await call('approval.answer', { id: approval.id, allow: true })
  await until('native desktop browser completed with screenshot', async () => (await call('chat.page', { conversation: bot.conversation })).items.some(i => i.kind === 'bot' && i.text.includes('已在本桌面打开团队浏览器') && i.text.includes('已附截图')), 60000)
  await call('bot.update', { id: bot.id, permission: 'readonly' })
  await call('chat.send', { conversation: bot.conversation, text: 'TOOL desktop_browser {"url":"https://example.com"}' })
  await until('read-only browser denied', async () => (await call('chat.page', { conversation: bot.conversation })).items.some(i => i.kind === 'bot' && /read.only/i.test(i.text)), 40000)
  await call('bot.update', { id: bot.id, permission: 'auto' })
  console.log('ok   real desktop browser, strict URL/app inputs, takeover, read-only policy, desktop prompt and no standalone MCP')

  const peer = await call('bot.create', { name: '共享电脑验证', role: 'Tester', permission: 'auto' })
  await call('desktop.open', { bot: peer.id })
  const peerStatus = await call('desktop.status', { bot: peer.id })
  assert.equal(peerStatus.container, container)
  assert.equal(peerStatus.shared, true)
  assert.notEqual(peerStatus.display, display)
  const runOn = (screen, command) => docker('exec', '-u', 'abc', '-e', `DISPLAY=${screen}`, container, 'bash', '-lc', command)
  docker('exec', container, 'sh', '-c', "printf '#!/bin/sh\\nprintf shared-software-ok' >/usr/local/bin/dotbot-shared-check; chmod 755 /usr/local/bin/dotbot-shared-check")
  await call('desktop.control', { bot: peer.id, user: true })
  await call('desktop.launch', { bot: peer.id, app: 'browser', url: fixtureUrl })
  await until('second screen browser without profile conflict', () => { try { return runOn(peerStatus.display, "xdotool search --onlyvisible --name '桌面浏览器验证'") } catch { return false } }, 60000)
  runOn(display, 'xdotool mousemove 101 102')
  runOn(peerStatus.display, 'xdotool mousemove 301 302')
  assert.match(runOn(display, 'xdotool getmouselocation --shell'), /X=101\nY=102/)
  assert.match(runOn(peerStatus.display, 'xdotool getmouselocation --shell'), /X=301\nY=302/)
  await call('desktop.control', { bot: peer.id, user: false })
  await call('desktop.control', { bot: bot.id, user: true })
  assert.equal((await call('desktop.status', { bot: peer.id })).user, false)
  const sharedCommand = 'printf "screen=%s bus=%s " "$DISPLAY" "$DBUS_SESSION_BUS_ADDRESS"; dotbot-shared-check; printf shared-file-ok >/config/shared-check.txt'
  await call('chat.send', { conversation: `bot:${peer.id}`, text: 'TOOL desktop_run ' + JSON.stringify({ command: sharedCommand }) })
  await until('peer tool command uses its screen and shared OS', async () => {
    for (const approval of (await call('team.state')).work.approvals) await call('approval.answer', { id: approval.id, allow: true })
    return (await call('chat.page', { conversation: `bot:${peer.id}` })).items.some(i => i.kind === 'bot' && i.text.includes(`screen=${peerStatus.display} bus=unix:`) && i.text.includes('shared-software-ok'))
  }, 60000)
  assert.equal(runOn(display, 'cat /config/shared-check.txt'), 'shared-file-ok')
  await call('desktop.control', { bot: bot.id, user: false })
  const unauthenticated = await fetch(`${app.base}/desktop/${peer.id}/`)
  assert.equal(unauthenticated.status, 401)
  docker('restart', '-t', '2', container)
  await call('desktop.open', { bot: bot.id })
  await call('desktop.open', { bot: peer.id })
  // The container restart also stops the local website fixture.
  docker('exec', '-d', '-u', 'abc', '-w', '/config', container, 'python3', '-m', 'http.server', '8844', '--bind', '127.0.0.1')
  assert.equal((await call('desktop.status', { bot: bot.id })).display, display)
  assert.equal((await call('desktop.status', { bot: peer.id })).display, peerStatus.display)
  assert.equal(runOn(peerStatus.display, 'cat /config/shared-check.txt; dotbot-shared-check'), 'shared-file-okshared-software-ok')
  await call('bot.delete', { id: peer.id })
  await until('deleted Bot screen stops', () => { try { runOn(peerStatus.display, 'xdotool getdisplaygeometry'); return false } catch { return true } }, 10000)
  assert.equal(docker('inspect', '-f', '{{.State.Running}}', container), 'true')
  assert.equal(runOn(display, 'xdotool getdisplaygeometry'), '1280 900')
  console.log('ok   two Bots share one OS/files, isolated browsers/cursors/DBus, independent takeover, restart and safe Bot deletion')

  await call('settings.lang', { lang: 'zh' })
  browserServer = await chromium.launchServer({ headless: true, executablePath: process.env.DOTBOT_CHROME || undefined })
  browserPids = descendants(browserServer.process().pid)
  browser = await chromium.connect(browserServer.wsEndpoint())
  context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, locale: 'zh-CN' })
  page = await context.newPage()
  const errors = []; page.on('pageerror', e => errors.push(e.message))
  await app.authorize(context); await page.goto(app.address)
  await page.locator('#d-status').filter({ hasText: '正在查看桌面' }).waitFor({ timeout: 180000 })
  assert.equal(await page.locator('#b-card').count(), 0)
  await page.locator('#conversation-menu > summary').click()
  assert.equal(await page.locator('#act-browser').count(), 0)
  assert.equal(await page.locator('#act-desktop').textContent(), '桌面与应用')
  await page.locator('#act-desktop').click()
  assert.equal(await page.locator('#panel-title').textContent(), '桌面与应用')
  assert.equal(await page.locator('#desktop-toolbar').isVisible(), true)
  assert.equal(await page.locator('[data-desktop-app="browser"]').isVisible(), true)
  assert.equal(await page.locator('#d-url').isVisible(), true)
  assert.equal(await page.locator('#b-view').count(), 0)
  const frameSource = await page.locator('#d-view iframe').getAttribute('src')
  await page.locator('#d-url').fill(fixtureUrl)
  await page.locator('#d-go').click()
  await page.locator('#d-view.popup').waitFor()
  await page.waitForFunction(() => document.querySelector('#d-status')?.textContent === '你已接管')
  assert.equal((await call('desktop.status', { bot: bot.id })).user, true)
  await page.locator('#d-giveback').click()
  await page.waitForFunction(() => !document.querySelector('#d-view').classList.contains('popup'))
  await page.locator('[data-desktop-app="files"]').click()
  await page.locator('#d-view.popup').waitFor()
  await page.waitForFunction(() => document.querySelector('#d-status')?.textContent === '你已接管')
  await page.locator('#d-giveback').click()
  assert.equal(await page.locator('#d-view iframe').getAttribute('src'), frameSource)
  await page.locator('[data-desktop-app="terminal"]').click()
  await page.locator('#d-view.popup').waitFor()
  await page.waitForFunction(() => document.querySelector('#d-status')?.textContent === '你已接管')
  await page.locator('#d-giveback').click()
  let failConnection = true
  await page.route('**/api/bot', route => {
    if (failConnection && route.request().postDataJSON()?.method === 'desktop.open') {
      failConnection = false
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ok: false, error: { message: '桌面测试连接失败' } }) })
    }
    return route.continue()
  })
  await page.locator('#d-reload').click()
  await page.locator('#d-retry').waitFor()
  assert.equal(await page.locator('#d-status').textContent(), '连接失败')
  await page.locator('#d-retry').click()
  await page.waitForFunction(() => document.querySelector('#d-status')?.textContent === '正在查看桌面')
  assert.equal(await page.locator('#d-view iframe').getAttribute('src'), frameSource)
  await page.unroute('**/api/bot')
  mkdirSync(join(root, 'test-results'), { recursive: true })
  // A visible canvas alone can still be a disconnected black stream. Require
  // actual decoded desktop pixels through the authenticated WebSocket proxy.
  const renderedDesktop = async () => {
    for (const frame of page.frames()) {
      if (frame === page.mainFrame()) continue
      const decoded = await frame.evaluate(() => {
        for (const source of document.querySelectorAll('canvas,video')) {
          const w = source.videoWidth || source.width, h = source.videoHeight || source.height
          if (!w || !h || source.tagName === 'VIDEO' && source.readyState < 2) continue
          try {
            const sample = document.createElement('canvas'); sample.width = 64; sample.height = 45
            const ctx = sample.getContext('2d'); ctx.drawImage(source, 0, 0, 64, 45)
            const pixels = ctx.getImageData(0, 0, 64, 45).data
            let lit = 0
            for (let i = 0; i < pixels.length; i += 4) if (pixels[i] + pixels[i+1] + pixels[i+2] > 90) lit++
            if (lit > 120) return true
          } catch {}
        }
        return false
      }).catch(() => false)
      if (decoded) return true
    }
    return false
  }
  await until('actual desktop frames render through the shared-computer proxy', renderedDesktop, 60000)
  await page.screenshot({ path: join(root, 'test-results/desktop-apps.png'), fullPage: true })
  await page.setViewportSize({ width: 390, height: 844 })
  await page.locator('#panel-close').click()
  await page.locator('#conversation-menu > summary').click()
  await page.locator('#act-desktop').click()
  await page.locator('#desktop-toolbar').waitFor()
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
  await page.locator('#d-control').click()
  await page.locator('#d-view.popup').waitFor()
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
  await page.locator('#d-giveback').click()
  await page.locator('.desktop-teach > summary').click()
  await page.locator('#d-teach-start').click()
  await page.locator('#d-view.popup').waitFor()
  const surface=await until('remote desktop video/canvas',async()=>{for(const f of page.frames()){if(f===page.mainFrame())continue;const found=f.locator('canvas:visible,video:visible').first();if(await found.isVisible().catch(()=>false))return found}},60000)
  await surface.click({position:{x:60,y:60},force:true})
  await page.keyboard.type('TEACH_PRIVATE_INPUT_7162')
  await page.keyboard.press('Tab')
  await until('sanitized demonstration events persisted',async()=>(await page.locator('#d-teach-count').textContent()).includes('步'),5000)
  await page.locator('#d-giveback').click()
  await page.locator('#d-teach-goal').fill('整理桌面软件输出，保存为参数化技能')
  await page.locator('#d-teach-finish').click()
  await until('desktop lesson skill saved',async()=>(await call('team.state')).skills.find(s=>s.name==='desktop-demo-fixture'),40000)
  const lessonItems=(await call('chat.page',{conversation:bot.conversation})).items;const lesson=lessonItems.find(i=>i.kind==='user'&&i.text?.includes('[桌面示范教学]'));assert.ok(lesson);assert.ok(lesson.text.includes('输入任务参数'),lesson.text);assert.ok(!JSON.stringify(lessonItems).includes('TEACH_PRIVATE_INPUT_7162'))
  assert.equal((await call('desktop.status',{bot:bot.id})).user,false)
  console.log('ok   real desktop canvas/video actions become a reusable skill; typed values are omitted and takeover is released')
  mkdirSync(join(home,'workspace'),{recursive:true});writeFileSync(join(home,'workspace/demo.json'),' {"value":1}')
  const trial=await call('skill.trial',{name:'desktop-demo-fixture',bot:bot.id,goal:'TOOL desktop_run '+JSON.stringify({command:`pwd; printf '{"value":2}' > demo.json`}),acceptance:'在桌面任务副本修改 JSON，再合并回桌面项目目录；不得调用本机终端。'})
  const cloudFlow=await until('cloud desktop project delivered',async()=>{
    for(const a of (await call('team.state')).work.approvals)await call('approval.answer',{id:a.id,allow:true})
    return(await call('workflow.list')).find(f=>f.id===trial&&['completed','needs_attention'].includes(f.status))
  },60000)
  assert.equal(cloudFlow.status,'completed',JSON.stringify(cloudFlow.verification));assert.equal(JSON.parse(readFileSync(join(home,'workspace/demo.json'),'utf8')).value,2)
  const cloudTask=(await call('task.list')).find(t=>t.workflow===trial);assert.ok(cloudTask.result.includes('/config/task-workspaces/'));assert.equal((await call('local.status')).enabled,false)
  console.log('ok   desktop commands use isolated project copies, merge and syntax validation without enabling local terminal')

  await page.setViewportSize({width:1440,height:1000})
  await page.locator('#input').fill('升级重连仍应保留这条草稿')
  await page.locator('#d-control').click()
  await page.locator('#d-view.popup').waitFor()
  let restoredTakeover=false
  const takeoverRequest=request=>{const body=request.postDataJSON?.();if(request.url().endsWith('/api/bot')&&body?.method==='desktop.control'&&body.params.user)restoredTakeover=true}
  // Limit parsing to API requests; desktop stream requests may be non-JSON.
  const onRequest=request=>{if(request.url().endsWith('/api/bot'))takeoverRequest(request)}
  page.on('request',onRequest)
  const oldInstance=await page.evaluate(()=>feedInstance)
  app.server.kill('SIGUSR2')
  await page.waitForFunction(previous=>feedInstance&&feedInstance!==previous&&document.querySelector('#link-state').classList.contains('live'),oldInstance,{timeout:40000})
  await until('user takeover renewal resumes after service replacement',()=>restoredTakeover,30000)
  await page.locator('#d-view.popup').waitFor()
  assert.equal((await call('desktop.status',{bot:bot.id})).user,true)
  await page.locator('#d-giveback').click()
  page.off('request',onRequest)
  assert.equal(await page.locator('#input').inputValue(),'升级重连仍应保留这条草稿')
  await page.locator('#files').setInputFiles({name:'upgrade-draft.txt',mimeType:'text/plain',buffer:Buffer.from('fixture attachment must survive UI update')})
  await page.waitForFunction(()=>attached.length===1&&!attached[0].pending&&Boolean(attached[0].src))
  const draftFile=await page.evaluate(()=>attached[0].src)
  await page.evaluate(()=>onEvent('hello',{serverInstance:feedInstance,assetRevision:'changed-assets-fixture'}))
  await page.locator('#reload-ui').click()
  await page.waitForFunction(()=>document.querySelector('#input').value==='升级重连仍应保留这条草稿')
  assert.equal(await page.evaluate(()=>attached[0].src),draftFile)
  assert.equal(await page.evaluate(()=>attached[0].name),'upgrade-draft.txt')
  console.log('ok   real service restart reconnects without losing draft; explicit UI update retains text and uploaded attachments')

  assert.deepEqual(errors, [])
  console.log('ok   desktop website/app toolbar, real shared frame, user takeover, Chinese labels and mobile layout')
} catch (error) {
  if (page && !page.isClosed()) { try { await page.screenshot({ path: join(root, 'test-results/desktop-ui-error.png'), fullPage: true }) } catch {} }
  if (container) {
    try { console.error(docker('exec', '-u', 'abc', '-e', `DISPLAY=${display}`, container, 'sh', '-c', 'ps -eo args | head -65; xdotool search --onlyvisible --name . getwindowname; curl -I http://127.0.0.1:8844/desktop-fixture.html')) } catch {}
    try { mkdirSync(join(root, 'test-results'), { recursive: true }); writeFileSync(join(root, 'test-results/desktop-error.jpg'), execFileSync('docker', ['exec', '-u', 'abc', '-e', `DISPLAY=${display}`, container, 'import', '-window', 'root', 'jpg:-'])) } catch {}
  }
  throw error
} finally {
  const outcomes = await Promise.allSettled([
    (async () => { try { await page?.close() } finally { try { await context?.close() } finally { try { await browser?.close() } finally { await browserServer?.close() } } } })(),
    app?.stop(),
  ])
  // Never touch pre-existing user desktops. Only the random Bot ID from this test.
  if (container) {
    try {
      const mounts = JSON.parse(docker('inspect', '--format', '{{json .Mounts}}', container))
      assert.ok(mounts.some(m => m.Source.startsWith(app.home + '/')), 'fixture container ownership must match the temporary home')
      docker('rm', '-f', container)
    } catch (error) { if (!error.stderr?.toString().includes('No such')) outcomes.push({ status: 'rejected', reason: error }) }
  }
  await until('headless browser cleanup', () => browserPids.every(pid => !alive(pid)), 5000)
  const failures = outcomes.filter(r => r.status === 'rejected').map(r => r.reason)
  if (failures.length) throw new AggregateError(failures, 'Desktop test cleanup failed')
  console.log('ok   task-owned browser and desktop container closed; existing user desktops untouched')
}
