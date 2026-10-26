#!/usr/bin/env node
// Exercise actual model process lifetimes, not only queued driver state.
import assert from 'node:assert/strict'
import {existsSync,readFileSync} from 'node:fs'
import {startServer,until} from './harness.mjs'
let app
try {
 app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 const {call}=app
 await call('settings.learning',{enabled:false})
 const chief=(await call('team.state')).bots[0], chat=chief.conversation
 const workers=[]
 for(let i=0;i<4;i++)workers.push(await call('bot.create',{name:`协作员${i}`,role:'Reviewer',model:`worker-model-${i}`}))
 const events=()=>existsSync(app.log+'.turns')?readFileSync(app.log+'.turns','utf8').trim().split('\n').map(JSON.parse):[]
 const starts=()=>events().filter(row=>row.event==='start')
 const items=conversation=>call('chat.page',{conversation}).then(p=>p.items)
 const allIdle=()=>until('all conversations idle',async()=>!Object.keys((await call('team.state')).work.runs).length)
 const active=key=>until('selected job accepts corrections',async()=>{const r=(await call('team.state')).work.runs[key];return r?.acceptingCorrections&&r})
 const upload=async(conversation,name,body)=>{
  const response=await fetch(`${app.base}/api/files?${new URLSearchParams({conversation,name})}`,{method:'POST',headers:{cookie:app.cookie,'content-type':'application/octet-stream'},body})
  assert.equal(response.status,200);return (await response.json()).file
 }
 const original=await upload(chat,'原始.txt','ORIGINAL_EVIDENCE'),extra=await upload(chat,'纠正.txt','CORRECTION_EVIDENCE')
 await call('chat.send',{conversation:chat,text:'AUTO_COORDINATE fleet_case PARALLEL_WORK SHARE_FILE INHERIT_FILES',files:[original.src]})
 const fleet=await until('four actual specialist models execute',async()=>{
  const tasks=(await call('task.list')).filter(t=>t.text.includes('fleet_case_worker'))
  return tasks.length===4&&tasks.every(t=>starts().some(row=>row.conversation===t.targetConversation))&&tasks
 })
 assert.equal(new Set(fleet.map(t=>t.target)).size,4)
 const lifetimes=events()
 for(const task of fleet){
  const start=lifetimes.find(row=>row.event==='start'&&row.conversation===task.targetConversation)
  assert.ok(!lifetimes.some(row=>row.event==='end'&&row.pid===start.pid),'all four processes must overlap')
  assert.deepEqual(task.inputs,[original.src])
 }
 await call('chat.correct',{conversation:chat,task:fleet[0].id,text:'CORRECT_TO selected_specialist',files:[extra.src]})
 const corrected=await until('selected task corrected',async()=>{const t=await call('task.get',{id:fleet[0].id});return t.status==='completed'&&t})
 assert.match(corrected.result,/Corrected: selected_specialist.*original=true; extra=true/)
 assert.equal(corrected.corrections.length,1)
 assert.deepEqual(new Set(corrected.inputs),new Set([original.src,extra.src]))
 await call('task.cancel',{id:fleet[1].id})
 assert.equal((await call('task.get',{id:fleet[1].id})).status,'cancelled')
 assert.equal((await call('task.get',{id:fleet[2].id})).status,'running')
 assert.equal((await call('task.get',{id:fleet[3].id})).status,'running')
 await until('other specialists finish normally',async()=>(await call('task.get',{id:fleet[2].id})).status==='completed'&&(await call('task.get',{id:fleet[3].id})).status==='completed')
 await allIdle()
 assert.ok((await items(chat)).some(i=>i.text?.includes('selected_specialist')))
 assert.equal((await call('workflow.list')).length,0)
 console.log('ok   four asynchronous specialists overlap; correction/stop target one task and keep attachments and siblings intact')

 // Occupy a Bot, enqueue two independent delegated sessions, then foreground
 // requests in its private chat and group seat. Its private chat and delegated work
 // share one worker; its group seat is a separate lane and runs alongside.
 const worker=workers[0], workerChat=`bot:${worker.id}`
 await call('chat.send',{conversation:workerChat,text:'DELAY 8000\npriority occupied'})
 const occupied=await active(workerChat)
 await until('occupied model launched',()=>starts().some(row=>row.conversation===workerChat&&row.prompt.includes('priority occupied')))
 const assign=async text=>{
  await call('chat.send',{conversation:chat,text:'TOOL message_bot '+JSON.stringify({bot:worker.name,text})})
  return until('background durably queued',async()=>{const t=(await call('task.list')).find(t=>t.text===text&&t.status==='queued');return t&&((await call('team.state')).work.runs[t.targetConversation]?.queued||0)>0&&t})
 }
 const bg1=await assign('DELAY 700\npriority_background_first')
 const bg2=await assign('priority_background_second')
 await call('chat.send',{conversation:workerChat,text:'DELAY 700\npriority_user_first'})
 const room=await call('room.create',{name:'优先级群聊',members:[worker.id],admin:worker.id})
 const group=`room:${room.id}`
 await call('chat.send',{conversation:group,text:'priority_user_group_second'})
 await until('group seat runs beside the occupied private chat',()=>starts().some(row=>row.conversation===`${group}:${worker.id}`))
 {
  const busy=starts().find(r=>r.conversation===workerChat&&r.prompt.includes('priority occupied')), seated=starts().find(r=>r.conversation===`${group}:${worker.id}`)
  const busyEnd=events().find(r=>r.event==='end'&&r.pid===busy.pid&&r.at>=busy.at)
  assert.ok(!busyEnd||seated.at<busyEnd.at,'group work does not wait for private work')
 }
 const cancelled=await assign('priority_cancelled_never_launch')
 await call('task.cancel',{id:cancelled.id})
 assert.ok(!starts().some(row=>row.conversation===bg1.targetConversation||row.conversation===bg2.targetConversation))
 await call('chat.correct',{conversation:workerChat,run:workerChat,since:occupied.since,text:'CORRECT_TO release_priority_slot'})
 await until('background work completed',async()=>(await call('task.get',{id:bg1.id})).status==='completed'&&(await call('task.get',{id:bg2.id})).status==='completed')
 await allIdle()
 const evidence=events(), order=starts().filter(row=>row.prompt.includes('priority_user_first')||row.conversation===bg1.targetConversation||row.conversation===bg2.targetConversation)
 assert.equal(order.length,3)
 assert.equal(order[0].conversation,workerChat)
 assert.equal(order[1].conversation,bg1.targetConversation)
 assert.equal(order[2].conversation,bg2.targetConversation)
 for(let i=1;i<order.length;i++){
  const previousEnd=evidence.find(row=>row.event==='end'&&row.pid===order[i-1].pid)
  assert.ok(previousEnd&&order[i].at>=previousEnd.at,'a Bot serializes its private chat and delegated work')
 }
 assert.ok(!starts().some(row=>row.conversation===cancelled.targetConversation),'cancelled waiting work must never launch')
 console.log('ok   group seats run beside private work; private user requests outrank older background delegations; same-priority work stays FIFO; cancelled queued work never launches')

 const groupFleet=await call('room.create',{name:'群内协作',members:[chief.id,...workers.map(b=>b.id)],admin:chief.id})
 const groupChat=`room:${groupFleet.id}`,groupFile=await upload(groupChat,'群内资料.txt','ORIGINAL_EVIDENCE')
 await call('chat.send',{conversation:groupChat,text:'AUTO_COORDINATE group_fleet PARALLEL_WORK SHARE_FILE INHERIT_FILES',files:[groupFile.src]})
 const tasks=await until('group specialists really overlap',async()=>{
  const tasks=(await call('task.list')).filter(t=>t.text.includes('group_fleet_worker'))
  return tasks.length===4&&tasks.every(t=>starts().some(row=>row.conversation===t.targetConversation))&&tasks
 })
 for(const t of tasks){assert.ok(t.targetConversation.startsWith(groupChat+':'));assert.deepEqual(t.inputs,[groupFile.src])}
 const groupEvidence=events()
 for(const t of tasks){const start=groupEvidence.find(row=>row.event==='start'&&row.conversation===t.targetConversation);assert.ok(!groupEvidence.some(row=>row.event==='end'&&row.pid===start.pid))}
 await call('chat.stop',{conversation:groupChat})
 await allIdle()
 for(const t of tasks)assert.equal((await call('task.get',{id:t.id})).status,'cancelled')
 for(const b of workers)assert.ok(!(await items(`bot:${b.id}`)).some(i=>i.text?.includes('group_fleet')||i.src===groupFile.src))
 const cli=readFileSync(app.log,'utf8').trim().split('\n').map(line=>JSON.parse(line).args)
 for(let i=0;i<workers.length;i++){
  const models=cli.filter(args=>args.includes('--system-prompt')&&args[args.indexOf('--system-prompt')+1].includes(`你是 ${workers[i].name}，`))
  assert.ok(models.length);assert.ok(models.every(args=>args[args.indexOf('--model')+1]===`worker-model-${i}`))
 }
 assert.equal((await call('workflow.list')).length,0)
 console.log('ok   group specialists overlap in isolated group sessions, preserve models/files and all stop without leaking into private chats')
}finally{await app?.stop()}
