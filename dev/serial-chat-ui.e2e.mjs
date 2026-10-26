#!/usr/bin/env node
import assert from 'node:assert/strict'
import {chromium} from 'playwright'
import {execFileSync} from 'node:child_process'
import {readFileSync,mkdirSync} from 'node:fs'
import {join} from 'node:path'
import {startServer,until,root} from './harness.mjs'
let app,server,browser,context,page,owned=[]
const descendants=pid=>{const rows=execFileSync('ps',['-eo','pid=,ppid=']).toString().trim().split('\n').map(r=>r.trim().split(/\s+/).map(Number)),ids=new Set([pid]);let n;do{n=ids.size;for(const[id,parent]of rows)if(ids.has(parent))ids.add(id)}while(n!==ids.size);return [...ids]}
const running=pid=>{try{return readFileSync(`/proc/${pid}/stat`,'utf8').split(') ')[1][0]!=='Z'}catch{return false}}
try{
  app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0',DOTBOT_LEARNING_DELAY_MS:'300'}})
  const {call}=app;await call('settings.lang',{lang:'zh'});await call('settings.learning',{enabled:false})
  const chief=(await call('team.state')).bots[0]
  const worker=await call('bot.create',{name:'协作助手',role:'Review'})
  server=await chromium.launchServer({headless:true,executablePath:process.env.DOTBOT_CHROME||undefined});owned=descendants(server.process().pid)
  browser=await chromium.connect(server.wsEndpoint());context=await browser.newContext({viewport:{width:1280,height:900},locale:'zh-CN'});page=await context.newPage()
  const errors=[];page.on('pageerror',e=>errors.push(e.message));await app.authorize(context); await page.goto(app.address)
  await page.waitForFunction(()=>view.team&&view.current)
  await call('chat.send',{conversation:chief.conversation,text:'TOOL message_bot '+JSON.stringify({bot:worker.name,text:'DELAY 3000\nordinary chat result'})})
  await until('worker executing',async()=>(await call('task.list')).some(t=>t.target===worker.id&&t.status==='running'))
  await page.waitForFunction(()=>view.work.tasks?.some(t=>t.status==='running'))
  assert.equal(await page.locator('#conversation-tasks').count(),0)
  assert.equal(await page.locator('[data-page="usage"]').count(),0)
  assert.equal(await page.locator('.delivery-card, .workflow-progress, [data-task-pause], [data-task-priority]').count(),0)
  await until('ordinary reply visible',async()=>(await call('chat.page',{conversation:chief.conversation})).items.some(i=>i.kind==='incoming'&&i.text.includes('ordinary chat result')))
  await page.waitForFunction(()=>view.items.some(i=>i.kind==='bot'&&i.text.includes('ordinary chat result')))
  await page.evaluate(()=>{view.items.push({id:'old-report',kind:'delivery',goal:'旧交付卡片',verification:{passed:true}});renderMessages({keepScroll:true})})
  assert.equal(await page.locator('[data-id="old-report"]').count(),0)
  await page.evaluate(()=>openPanel('activity'))
  assert.equal(await page.locator('[data-task-pause], [data-task-priority], [data-workflow-retry]').count(),0)
  await page.evaluate(()=>closePanel())
  const artifacts=join(root,'test-results');mkdirSync(artifacts,{recursive:true})
  await page.screenshot({path:join(artifacts,'serial-chat-desktop.png'),animations:'disabled'})
  await page.setViewportSize({width:390,height:844});await page.waitForFunction(()=>document.querySelector('#sidebar').getBoundingClientRect().right<=1)
  assert.ok(await page.locator('#main').evaluate(el=>el.scrollWidth<=el.clientWidth+1))
  await page.screenshot({path:join(artifacts,'serial-chat-mobile.png'),animations:'disabled'})
  assert.deepEqual(errors,[]);console.log('ok   ordinary delegated chat fits desktop/mobile; removed banner, report cards, prices and scheduling controls remain absent')
}finally{
  if(server&&running(server.process().pid))owned=[...new Set([...owned,...descendants(server.process().pid)])]
  try{await page?.close()}finally{try{await context?.close()}finally{try{await browser?.close()}finally{await server?.close()}}}
  for(const pid of owned.filter(running)){try{process.kill(pid,'SIGTERM')}catch{}}
  let remaining=[];try{await until('browser cleanup',()=>owned.every(pid=>!running(pid)),5000)}catch{remaining=owned.filter(running)}
  await app?.stop();assert.deepEqual(remaining,[],`Task-owned browser PIDs remain: ${remaining}`);console.log('ok   all task-owned browser sessions/processes and test service cleaned up')
}
