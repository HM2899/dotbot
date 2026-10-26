#!/usr/bin/env node
// Browser lifecycle and cookie regressions using MCP fixtures, no real browser/account.
import assert from 'node:assert/strict'
import { mkdtempSync, symlinkSync, readFileSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { once } from 'node:events'
import { startServer, root, until } from './harness.mjs'
const home = mkdtempSync(join(tmpdir(), 'dotbot-browser-check-'))
const fixture = join(home, 'playwright-mcp.mjs')
symlinkSync(join(root, 'dev/fake-browser.mjs'), fixture)
const options = { browser: true, directory: home, keepHome: true, browserCommand: `${process.execPath} ${fixture}` }
let app, replacement, pid
const alive = pid => { try { return readFileSync(`/proc/${pid}/stat`, 'utf8').split(') ')[1][0] !== 'Z' } catch { return false } }
try {
  app = await startServer(options)
  const bot = (await app.call('team.state')).bots[0]
  await Promise.all(Array.from({ length: 5 }, () => app.call('browser.peek', { bot: bot.id })))
  const profile = join(home, 'team/browsers', bot.id), pidFile = join(home, 'team/browsers', `${bot.id}.pid`)
  pid = Number(readFileSync(pidFile, 'utf8'))
  assert.match(await app.call('browser.share', { bot: bot.id }), /Shared 1 cookies/)
  const cookiesFile = join(home, 'team/browsers/shared-cookies.json')
  const saved = readFileSync(cookiesFile, 'utf8')
  writeFileSync(join(profile, 'empty-cookies'), '')
  await assert.rejects(app.call('browser.share', { bot: bot.id }), /no saved logins/)
  assert.equal(readFileSync(cookiesFile, 'utf8'), saved, 'empty context must not overwrite existing shared logins')
  rmSync(join(profile, 'empty-cookies'))
  const exit = once(app.server, 'exit')
  app.server.kill('SIGKILL'); await exit
  assert.equal(alive(pid), true, 'fixture simulates a browser left by an old server crash')
  // Simulate an old PID file overwritten by a subsequent failed launch.
  writeFileSync(pidFile, '99999999')
  replacement = await startServer(options)
  assert.ok((await replacement.call('browser.peek', { bot: bot.id })).image)
  assert.equal(readFileSync(pidFile, 'utf8'), '99999999', 'reuses profile owner without launching a second browser')
  assert.match(await replacement.call('browser.share', { bot: bot.id }), /Shared 1 cookies/)
  const records = readFileSync(join(profile, 'fixture-calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse)
  assert.ok(records.filter(r => r.name === 'browser_take_screenshot').length >= 6)
  // Every page step comes back with the page, done, failed or refused; the chat gets the last one.
  // The fake model repeats each tool result in its reply; the fixture logs every screenshot taken.
  const chat = bot.conversation
  const page = async conversation => (await replacement.call('chat.page', { conversation })).items
  const shots = () => readFileSync(join(profile, 'fixture-calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse).filter(r => r.name === 'browser_take_screenshot').length
  const step = async text => {
    const before = (await page(chat)).length, taken = shots()
    await replacement.call('chat.send', { conversation: chat, text })
    await until('step answered', async () => (await page(chat)).slice(before).some(i => i.kind === 'bot'))
    await new Promise(resolve => setTimeout(resolve, 300))
    const items = (await page(chat)).slice(before)
    return { reply: items.find(i => i.kind === 'bot').text, shot: items.find(i => i.kind === 'file'), taken: shots() - taken }
  }
  let done = await step('TOOL mcp__browser__browser_click {"element":"Submit"}')
  assert.match(done.reply, /### Result\nDone\n已附操作后的页面截图/); assert.ok(done.taken >= 1)
  assert.match(done.shot.caption, /操作后的画面/); assert.equal(done.shot.mime, 'image/png')
  let failed = await step('TOOL mcp__browser__browser_click {"element":"Missing","fail":true}')
  assert.match(failed.reply, /Element not found\n已附当前页面截图：先看截图判断失败原因/); assert.ok(failed.taken >= 1)
  assert.match(failed.shot.caption, /未成功/)
  const look = await step('TOOL mcp__browser__browser_snapshot {}')
  assert.equal(look.shot, undefined, 'looking at the page is not an action and posts no screenshot')
  assert.doesNotMatch(look.reply, /已附/)
  await replacement.call('browser.control', { bot: bot.id, user: true })
  const refused = await step('TOOL mcp__browser__browser_click {"element":"Submit"}')
  assert.match(refused.reply, /The user has control[\s\S]*已附当前页面截图/); assert.ok(refused.taken >= 1, 'a refused step still shows the page')
  assert.match(refused.shot.caption, /未成功/)
  await replacement.call('browser.control', { bot: bot.id, user: false })
  console.log('ok   browser steps answer with the page whether they succeed, fail or are refused; the last one reaches the chat')
  await replacement.stop()
  await until('adopted browser closed', () => !alive(pid))
  console.log('ok   concurrent browser startup, quoted cookies, empty-cookie preservation, orphan profile reattachment and shutdown')
} finally {
  await Promise.allSettled([app?.stop(), replacement?.stop()])
  if (pid && alive(pid)) {
    // Only this test's recorded fixture process group is eligible for cleanup.
    try { process.kill(-pid, 'SIGTERM') } catch {}
    await until('fixture cleanup', () => !alive(pid), 5000)
  }
  rmSync(home, { recursive: true, force: true })
  assert.ok(!pid || !alive(pid), `Fixture PID ${pid} remains`)
}
