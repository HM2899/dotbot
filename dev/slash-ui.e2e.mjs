#!/usr/bin/env node
// "/" commands in the composer and reply rating buttons that only show on hover.
import assert from 'node:assert/strict'
import {chromium} from 'playwright'
import {startServer,until} from './harness.mjs'
let app,browser
try{
  app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  const {call}=app;await call('settings.lang',{lang:'zh'});await call('settings.learning',{enabled:false})
  await call('skill.save',{name:'weekly-report',description:'整理本周工作并写成周报',text:'# 周报\n\n按项目列出本周完成的工作、风险和下周计划。'})
  const chief=(await call('team.state')).bots[0]
  browser=await chromium.launch({headless:true,executablePath:process.env.DOTBOT_CHROME||undefined})
  const context=await browser.newContext({viewport:{width:1280,height:900},locale:'zh-CN'}),page=await context.newPage()
  const errors=[];page.on('pageerror',e=>errors.push(e.message));await app.authorize(context);await page.goto(app.address)
  await page.waitForFunction(()=>view.team&&view.current)

  await call('chat.send',{conversation:chief.conversation,text:'rating fixture'})
  await page.waitForFunction(()=>view.items.some(i=>i.kind==='bot'&&i.text))
  const reply=page.locator('.msg.bot').last(),good=reply.locator('.rate-good')
  await good.hover({force:true});await reply.hover();await good.click()
  await page.waitForFunction(()=>document.querySelector('.msg.bot:last-of-type .rate-good.on, .msg.bot .rate-good.on'))
  await page.mouse.move(5,5)
  await until('rated button fades out',async()=>await page.locator('.msg.bot .rate-good.on').last().evaluate(el=>getComputedStyle(el).opacity)==='0',3000)
  await reply.hover();await until('rated button back on hover',async()=>await page.locator('.msg.bot .rate-good.on').last().evaluate(el=>getComputedStyle(el).opacity)==='1',3000)

  const input=page.locator('#input'),menu=page.locator('#slash-menu')
  await input.click();await input.press('/')
  await menu.waitFor()
  const names=await menu.locator('strong').allTextContents()
  for(const name of ['/new','/search','/memory','/clear','/settings','/weekly-report'])assert.ok(names.includes(name),`missing ${name} in ${names}`)
  assert.ok(!names.includes('/stop'),'stop only shows while work runs')
  await input.fill('/wee');assert.deepEqual(await menu.locator('strong').allTextContents(),['/weekly-report'])
  await input.fill('/zzz');assert.ok(await menu.isHidden())
  await input.fill('/');await input.press('Escape');assert.ok(await menu.isHidden())

  // Enter runs a chat action and sends nothing.
  const before=(await call('chat.page',{conversation:chief.conversation})).items.length
  await input.fill('/mem');await input.press('Enter')
  await page.waitForFunction(()=>view.panel==='memory');assert.equal(await input.inputValue(),'')
  await input.fill('/search fixture');await input.press('Enter')
  await page.waitForFunction(()=>!document.querySelector('#chat-search').hidden&&document.querySelector('#chat-search-input').value==='fixture')
  await input.click();await input.fill('/new');await input.press('ArrowDown');await input.press('ArrowUp');await input.press('Enter')
  await until('fresh conversation notice',async()=>(await call('chat.page',{conversation:chief.conversation})).items.some(i=>i.kind==='notice'&&/fresh conversation/.test(i.text)))
  assert.equal((await call('chat.page',{conversation:chief.conversation})).items.filter(i=>i.kind==='user').length,1,'commands are never sent')

  // A skill completes into the composer and is sent as a request to use it.
  await input.fill('/weekly');await input.press('Enter')
  assert.equal(await input.inputValue(),'/weekly-report ');assert.ok(await menu.isHidden())
  await input.pressSequentially('本周做了登录页');await input.press('Enter')
  await until('skill request sent',async()=>(await call('chat.page',{conversation:chief.conversation})).items.some(i=>i.kind==='user'&&i.text==='使用技能 weekly-report：本周做了登录页'))
  await page.waitForFunction(()=>document.querySelector('#input').value==='')
  // Text that only looks like a path is an ordinary message.
  await input.fill('/home/me/notes.txt 是什么');await input.press('Enter')
  await until('path message sent',async()=>(await call('chat.page',{conversation:chief.conversation})).items.some(i=>i.kind==='user'&&i.text==='/home/me/notes.txt 是什么'))
  assert.ok(before>0)
  assert.deepEqual(errors,[]);console.log('ok   rated replies hide off hover; / lists actions and skills, runs actions locally, sends skills as requests')
}finally{await browser?.close();await app?.stop()}
