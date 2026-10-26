#!/usr/bin/env node
import assert from 'node:assert/strict'
import { chromium } from 'playwright'
import { execFileSync } from 'node:child_process'
import { readFileSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until, root } from './harness.mjs'

let app, server, browser, context, page, owned=[]
const descendants=pid=>{
  const rows=execFileSync('ps',['-eo','pid=,ppid=']).toString().trim().split('\n').map(row=>row.trim().split(/\s+/).map(Number)), ids=new Set([pid]);let before
  do{before=ids.size;for(const [id,parent] of rows)if(ids.has(parent))ids.add(id)}while(before!==ids.size)
  return [...ids]
}
const running=pid=>{try{return readFileSync(`/proc/${pid}/stat`,'utf8').split(') ')[1][0]!=='Z'}catch{return false}}
try {
  app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
  const {call}=app
  await call('settings.lang',{lang:'zh'});await call('settings.learning',{enabled:false})
  const chief=(await call('team.state')).bots[0], chat=chief.conversation
  const worker=await call('bot.create',{name:'资料核查 <A>',role:'Reviewer',model:'worker-model'})
  server=await chromium.launchServer({headless:true,executablePath:process.env.DOTBOT_CHROME||undefined})
  owned=descendants(server.process().pid)
  browser=await chromium.connect(server.wsEndpoint())
  context=await browser.newContext({viewport:{width:1280,height:900},locale:'zh-CN'})
  page=await context.newPage()
  const errors=[];page.on('pageerror',error=>errors.push(error.message))
  await app.authorize(context); await page.goto(app.address)
  await page.locator(`#bots [data-open="${chat}"]`).click()
  await page.waitForFunction(conversation=>view.current===conversation&&!view.conversationLoading,chat)
  const opacity=locator=>locator.evaluate(el=>Number(getComputedStyle(el).opacity))
  const idle=()=>until('chief idle',async()=>!(await call('team.state')).work.runs[chat])
  const items=async conversation=>(await call('chat.page',{conversation})).items
  const prompts=()=>readFileSync(`${app.log}.turns`,'utf8').trim().split('\n').map(line=>JSON.parse(line)).filter(entry=>entry.event==='start')
  const openChat=async conversation=>{
    await page.evaluate(conversation=>open(conversation),conversation)
    await page.waitForFunction(conversation=>view.current===conversation&&!view.conversationLoading,conversation)
  }
  const noModeButtons=async()=>{
    assert.equal(await page.locator('#task-focus, [data-correction-target], [data-correction-select], [data-task-correct]').count(),0)
    assert.equal(await page.getByText('纠正当前任务',{exact:true}).count(),0)
    assert.equal(await page.getByText('新任务',{exact:true}).count(),0)
  }
  const artifacts=join(root,'test-results');mkdirSync(artifacts,{recursive:true})

  // A message sent while the Bot works joins that work; the Bot is told to decide
  // whether it amends the task or is a separate request for afterwards.
  await call('chat.send',{conversation:chat,text:'DELAY 8000\noriginal task'})
  await until('original task accepts messages',async()=>(await call('team.state')).work.runs[chat]?.acceptingCorrections)
  await noModeButtons()
  assert.equal(await page.locator('#send').getAttribute('title'),'发送')
  await page.locator('#input').fill('CORRECT_TO auto_ui')
  await page.locator('#input').press('Enter')
  await until('working Bot handled the new message',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('Corrected: auto_ui')))
  const steered=(await items(chat)).find(i=>i.kind==='user'&&i.text==='CORRECT_TO auto_ui')
  assert.equal(steered.steered,true);assert.ok(!steered.correction)
  assert.ok(prompts().some(p=>p.prompt.includes('[任务进行中收到的新消息]')&&p.prompt.includes('结合当前任务及新消息的实际含义处理')))
  await page.locator('#messages .msg.user').filter({hasText:'由它判断是补充还是新任务'}).waitFor()
  await idle()
  await page.locator('#input').fill('plain_idle_message')
  await page.locator('#input').press('Enter')
  await until('idle message sent normally',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('plain_idle_message')))
  assert.ok(!(await items(chat)).find(i=>i.text==='plain_idle_message').steered)
  await idle()
  console.log('ok   no correction/new-task buttons; a message during work goes to that work and the Bot decides; idle messages send normally')

  // Before interrupting, a message is classified: a progress question is answered from the
  // work's state and a separate request waits in line; neither stops the running task.
  await call('chat.send',{conversation:chat,text:'DELAY 6000\ntriage original'})
  const triageRun=await until('triage work running',async()=>{const run=(await call('team.state')).work.runs[chat];return run?.acceptingCorrections&&run})
  await page.waitForFunction(()=>workingTarget())
  await page.locator('#input').fill('做到哪了 TRIAGE_PROGRESS'); await page.locator('#input').press('Enter')
  await until('progress answered',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.progress&&i.text.startsWith('PROGRESS_ANSWER')))
  await page.waitForFunction(()=>workingTarget())
  await page.locator('#input').fill('另一件事 TRIAGE_NEW separate_request'); await page.locator('#input').press('Enter')
  await until('new request queued',async()=>(await call('team.state')).work.runs[chat]?.queued===1)
  assert.equal((await call('team.state')).work.runs[chat].since,triageRun.since,'the original work was not restarted')
  await until('original finished uninterrupted',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('triage original')&&!i.text.includes('Corrected')))
  await until('separate request answered after it',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('separate_request')))
  await idle()
  console.log('ok   progress questions are answered and new requests queued without interrupting the running work')

  // Delegated work: the coordinator's chat routes the message to the running worker task.
  await call('chat.send',{conversation:chat,text:`TOOL message_bot ${JSON.stringify({bot:worker.name,text:'DELAY 8000\nworker current job'})}`})
  const task=await until('worker running',async()=>(await call('task.list')).find(t=>t.target===worker.id&&t.status==='running'))
  await noModeButtons()
  await page.setViewportSize({width:390,height:844})
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1))
  await page.screenshot({path:join(artifacts,'auto-steer-mobile.png'),animations:'disabled'})
  await page.setViewportSize({width:1280,height:900})
  // The coordinator is idle while its worker runs: the message goes to the coordinator, which
  // forwards an amendment to the same task with correct_task instead of a new delegation.
  await idle(); await page.waitForFunction(()=>!runOf(view.current))
  assert.equal(await page.evaluate(()=>workingTarget()),undefined)
  await page.locator('#input').fill(`TOOL correct_task ${JSON.stringify({id:task.id,text:'CORRECT_TO delegated_ui'})}`)
  await page.locator('#send').click()
  await until('same delegated task completed',async()=>(await call('task.get',{id:task.id})).status==='completed')
  assert.equal((await call('task.list')).length,1)
  assert.match((await call('task.get',{id:task.id})).result,/Corrected: delegated_ui/)
  await idle()
  console.log('ok   messages during delegated work go to the coordinator, which forwards amendments to the same task')

  const room=await call('room.create',{name:'自动判断群聊',members:[chief.id],admin:chief.id}), group=`room:${room.id}`
  await openChat(group)
  await call('chat.send',{conversation:group,text:'group original task\nDELAY 8000'})
  await until('group work accepts messages',async()=>Object.entries((await call('team.state')).work.runs).some(([key,run])=>key.startsWith(group)&&run.acceptingCorrections))
  await noModeButtons()
  await page.locator('#input').fill('CORRECT_TO group_auto')
  await page.locator('#send').click()
  await until('group reply',async()=>(await items(group)).some(i=>i.kind==='bot'&&i.text.includes('Corrected: group_auto')))
  await until('group idle',async()=>!Object.keys((await call('team.state')).work.runs).some(key=>key===group||key.startsWith(group+':')))
  console.log('ok   group chats route messages to the running member automatically')

  // Work that ends while the user types never loses the message: it becomes a new one.
  await openChat(chat)
  await call('chat.send',{conversation:chat,text:'DELAY 1500\nfinish while drafting'})
  await until('draft work running',async()=>(await call('team.state')).work.runs[chat]?.acceptingCorrections)
  await page.waitForFunction(()=>workingTarget()) // the page has the running work
  const stale=await page.evaluate(()=>workingTarget())
  assert.ok(stale)
  await idle()
  await page.evaluate(target=>call('chat.correct',{conversation:view.current,text:'late_draft',files:[],run:target.run,since:target.since,auto:true}),stale)
  await until('late draft sent as new message',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('late_draft')))
  await idle()
  console.log('ok   a message for work that already ended is sent as a new message')

  // Clearing history: refused while working, then removes every message here only.
  await call('chat.send',{conversation:`bot:${worker.id}`,text:'kept_in_other_chat'})
  await until('other chat replied',async()=>(await items(`bot:${worker.id}`)).some(i=>i.kind==='bot'))
  await call('chat.send',{conversation:chat,text:'DELAY 8000\nbusy while clearing'})
  await until('busy',async()=>(await call('team.state')).work.runs[chat])
  await assert.rejects(call('chat.clear',{conversation:chat}),/正在处理任务/)
  await call('chat.stop',{conversation:chat});await idle()
  page.once('dialog',dialog=>dialog.accept())
  await page.locator('#conversation-menu summary').click()
  await page.locator('#act-clear').click()
  await until('history cleared',async()=>(await items(chat)).length===0)
  await until('page emptied',async()=>await page.locator('#messages .msg').count()===0)
  assert.ok((await items(`bot:${worker.id}`)).some(i=>i.text==='kept_in_other_chat'),'other conversations are untouched')
  await page.locator('#input').fill('after_clear')
  await page.locator('#input').press('Enter')
  await until('chat works after clearing',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('after_clear')))
  assert.equal(prompts().filter(p=>p.conversation===chat).at(-1).prompt.includes('original task'),false)
  await call('chat.clear',{conversation:group})
  assert.equal((await items(group)).length,0)
  assert.deepEqual(errors,[])
  console.log('ok   clear chat history is refused while working, empties only this conversation (including groups) and chatting continues')
  await until('idle before touch test',async()=>!(await call('team.state')).work.runs[chat])
  // Reuse the browser process, with one context/page alive at a time.
  await page.close();await context.close()
  context=await browser.newContext({viewport:{width:390,height:844},isMobile:true,hasTouch:true,locale:'zh-CN'})
  page=await context.newPage()
  page.on('pageerror',error=>errors.push(error.message))
  await app.authorize(context); await page.goto(app.address)
  await page.locator('#toggle-sidebar').click()
  await page.locator(`#bots [data-open="${chat}"]`).click()
  await page.waitForFunction(conversation=>view.current===conversation&&!view.conversationLoading,chat)
  await call('chat.send',{conversation:chat,text:'DELAY 8000\ntouch stop test'})
  const touchStop=page.locator('#messages .msg.user .stop-btn')
  await touchStop.waitFor({state:'attached'})
  assert.equal(await page.evaluate(()=>matchMedia('(hover: none)').matches),true)
  await until('touch actions stay visible',async()=>await opacity(touchStop)===1&&await opacity(page.locator('#messages .msg.user .edit-btn').last())===1)
  await touchStop.tap()
  await until('touch Stop ends request',async()=>!(await call('team.state')).work.runs[chat])
  assert.deepEqual(errors,[])
  console.log('ok   phones retain visible, tappable Stop and Edit controls without hover')
} finally {
  if(server&&running(server.process().pid))owned=[...new Set([...owned,...descendants(server.process().pid)])]
  try{await page?.close()}finally{try{await context?.close()}finally{try{await browser?.close()}finally{await server?.close()}}}
  for(const pid of owned.filter(running)){try{process.kill(pid,'SIGTERM')}catch{}}
  let remaining=[]
  try{await until('task browser cleanup',()=>owned.every(pid=>!running(pid)),5000)}catch{remaining=owned.filter(running)}
  await app?.stop()
  assert.deepEqual(remaining,[],`Browser cleanup failed for PIDs: ${remaining.join(', ')}`)
  console.log('ok   all task-owned browser resources and correction test service closed')
}
