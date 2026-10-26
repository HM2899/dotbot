#!/usr/bin/env node
// Real scheduler, task receipts, resumed model sessions and attachment authorization.
import assert from 'node:assert/strict'
import { readFileSync, existsSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until } from './harness.mjs'

let app
try {
  app = await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined})
  const {base, cookie} = app
  const call=async(method,params={})=>{
    const response=await fetch(`${base}/api/bot`,{method:'POST',headers:{cookie,'content-type':'application/json'},body:JSON.stringify({method,params}),signal:AbortSignal.timeout(20000)}).catch(error=>{throw new Error(`${method} timed out or disconnected`,{cause:error})})
    const result=await response.json()
    if(!result.ok)throw new Error(`${method}: ${result.error.message}`)
    return result.value
  }
  await call('settings.learning',{enabled:false})
  const chief=(await call('team.state')).bots[0], chat=chief.conversation
  const worker=await call('bot.create',{name:'纠正执行员',role:'Reviewer',model:'worker-model'})
  const workerChat=`bot:${worker.id}`
  const items=async conversation=>(await call('chat.page',{conversation})).items
  const state=()=>call('team.state')
  const idle=conversation=>until('conversation idle',async()=>!(await state()).work.runs[conversation])
  const active=conversation=>until('correctable run',async()=>{const run=(await state()).work.runs[conversation];return run?.acceptingCorrections&&run})
  const upload=async(conversation,name,body)=>{
    const response=await fetch(`${base}/api/files?${new URLSearchParams({conversation,name})}`,{method:'POST',headers:{cookie,'content-type':'application/octet-stream'},body})
    assert.equal(response.status,200);return (await response.json()).file
  }
  const original=await upload(chat,'原始资料.txt','ORIGINAL_EVIDENCE')
  const extra=await upload(chat,'更正资料.txt','CORRECTION_EVIDENCE')
  const picture=await upload(chat,'参考.png',Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=','base64'))
  const tool=(name,args)=>`TOOL ${name} ${JSON.stringify(args)}`
  await call('chat.send',{conversation:chat,text:'DELAY 8000\nstandalone original request',files:[original.src,picture.src]})
  const run=await active(chat)
  await call('chat.send',{conversation:chat,text:'independent queued request'})
  await call('chat.correct',{conversation:chat,run:chat,since:run.since,text:'CORRECT_TO standalone',files:[extra.src]})
  await until('standalone corrected before the old eight second delay',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text==='Corrected: standalone; images=1 pdf=0; original=true; extra=true'),6000)
  await idle(chat)
  const direct=await items(chat)
  assert.ok(direct.some(i=>i.kind==='bot'&&i.text.includes('independent queued request')),'queued unrelated work must survive')
  assert.equal(direct.filter(i=>i.correction).length,1)
  assert.ok(!direct.some(i=>i.kind==='notice'&&i.error),'correction is not a failed or cancelled task')
  assert.ok(!direct.some(i=>i.kind==='bot'&&i.text.includes('standalone original request')),'obsolete answer must be discarded')
  await assert.rejects(call('chat.correct',{conversation:chat,run:chat,since:run.since,text:'CORRECT_TO stale'}),/结束|就绪|收尾/)
  console.log('ok   an in-flight private task resumes with its native/text inputs, accepts new attachments and preserves unrelated queued work')

  const rosterBefore=(await state()).bots.length
  await call('chat.send',{conversation:chat,text:'请委派核查 DELEGATION_SLOW\nNO_MATCH'})
  const planning=await until('delegation selection in progress',async()=>{const r=(await state()).work.runs[chat];return r?.activity==='allocating'&&r})
  await call('chat.correct',{conversation:chat,run:chat,since:planning.since,text:'不要委派，CORRECT_TO changed_before_dispatch'})
  await until('planning corrected promptly',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('Corrected: changed_before_dispatch')),4000)
  await idle(chat)
  assert.equal((await state()).bots.length,rosterBefore)
  assert.equal((await call('task.list')).length,0)
  console.log('ok   a correction during allocation stops the obsolete plan before it can create or dispatch another Bot')

  await call('settings.local-computer',{enabled:true})
  await call('bot.update',{id:chief.id,permission:'ask'})
  await call('chat.send',{conversation:chat,text:tool('local_run',{command:'touch stale-command-must-not-run',cwd:join(app.home,'work')})})
  const approval=await until('old task approval',async()=>(await state()).work.approvals.find(a=>a.conversation===chat))
  const waiting=(await state()).work.runs[chat]
  await call('chat.correct',{conversation:chat,run:chat,since:waiting.since,text:'取消旧指令，CORRECT_TO approval_replaced'})
  await until('corrected approval task complete',async()=>(await items(chat)).some(i=>i.kind==='bot'&&i.text.includes('Corrected: approval_replaced')))
  assert.ok(!(await state()).work.approvals.some(a=>a.id===approval.id))
  await assert.rejects(call('approval.answer',{id:approval.id,allow:true}),/no longer open/)
  assert.equal(existsSync(join(app.home,'work/stale-command-must-not-run')),false)
  await idle(chat)
  await call('bot.update',{id:chief.id,permission:'auto'})
  await call('settings.local-computer',{enabled:false})
  console.log('ok   replacing a waiting task revokes its obsolete approval and cannot execute the old command later')

  // Keep the worker occupied so the selected delegated job can be corrected in its queue.
  await call('chat.send',{conversation:workerChat,text:'DELAY 3000\nworker unrelated occupied turn'})
  await active(workerChat)
  await call('chat.send',{conversation:chat,text:tool('message_bot',{bot:worker.name,text:'DELAY 8000\nqueued delegated job'}),files:[original.src]})
  const queued=await until('delegation enqueued on isolated session',async()=>{const t=(await call('task.list')).find(t=>t.text.includes('queued delegated job'));return t&&((await state()).work.runs[t.targetConversation]?.queued||0)>0&&t})
  assert.equal((await call('task.get',{id:queued.id})).status,'queued')
  await call('chat.correct',{conversation:chat,task:queued.id,text:'CORRECT_TO queued'})
  const done=await until('corrected queued job completes',async()=>{const t=await call('task.get',{id:queued.id});return t.status==='completed'&&t})
  assert.equal(done.id,queued.id)
  assert.match(done.result,/Corrected: queued/)
  assert.equal(done.corrections.length,1)
  await until('unrelated worker finishes independently',async()=>(await items(workerChat)).some(i=>i.kind==='bot'&&i.text.includes('worker unrelated occupied turn')))
  await idle(chat);await idle(workerChat)
  console.log('ok   correcting a queued delegated job retains its receipt and does not interrupt another job on that worker')

  const before=new Set((await call('task.list')).map(t=>t.id))
  await call('chat.send',{conversation:chat,text:`请委派完成资料核查\nREUSE_BOT ${worker.id}\nDELAY 8000\nAUTO_RESULT steering_worker`,files:[original.src,picture.src]})
  const delegated=await until('worker delegated run',async()=>(await call('task.list')).find(t=>!before.has(t.id)&&t.status==='running'))
  await active(delegated.targetConversation)
  await until('isolated session initialized',()=>JSON.parse(readFileSync(join(app.home,'team/teambot.json'),'utf8')).sessions?.[delegated.targetConversation])
  await assert.rejects(call('chat.correct',{conversation:`bot:${(await call('bot.create',{name:'无关 Bot'})).id}`,task:delegated.id,text:'CORRECT_TO forbidden'}),/不属于/)
  await call('chat.correct',{conversation:chat,task:delegated.id,text:'CORRECT_TO first\nCORRECTION_HOLD',files:[extra.src]})
  await until('first correction consumed',async()=>(await state()).work.runs[delegated.targetConversation]?.activity==='correcting')
  await call('chat.correct',{conversation:workerChat,task:delegated.id,text:'CORRECT_TO latest'})
  const finished=await until('running task corrected twice',async()=>{const t=await call('task.get',{id:delegated.id});return t.status==='completed'&&t})
  assert.match(finished.result,/Corrected: latest; images=1 pdf=0; original=true; extra=true/)
  assert.equal(finished.corrections.length,2)
  assert.deepEqual(new Set(finished.inputs),new Set([original.src,picture.src,extra.src]))
  await idle(chat);await idle(workerChat)
  assert.equal((await call('task.list')).filter(t=>!before.has(t.id)).length,1,'corrections must not create replacement tasks')
  const receipts=(await call('recovery.list')).filter(w=>w.task===delegated.id)
  assert.equal(receipts.length,1,'the same durable execution receipt must finish')
  assert.match((await call('recovery.get',{id:receipts[0].id})).text,/CORRECT_TO latest/)
  const launches=readFileSync(app.log,'utf8').trim().split('\n').map(line=>JSON.parse(line).args).filter(args=>args.includes('--system-prompt')&&args[args.indexOf('--system-prompt')+1].includes(`你是 ${worker.name}，`))
  assert.ok(launches.some(args=>args.includes('--resume')))
  assert.ok(launches.every(args=>args[args.indexOf('--model')+1]==='worker-model'))
  await assert.rejects(call('chat.correct',{conversation:chat,task:delegated.id,text:'CORRECT_TO too_late'}),/已结束/)
  console.log('ok   repeated corrections keep one delegated task, one receipt, the worker model, attachments, resume context and the original result route')

  const room=await call('room.create',{name:'纠正群聊',members:[chief.id,worker.id],admin:chief.id}), group=`room:${room.id}`
  await call('chat.send',{conversation:group,text:`请委派核查\nREUSE_BOT ${worker.id}\nDELAY 8000\nAUTO_RESULT group_steering`})
  const groupTask=await until('group worker running',async()=>(await call('task.list')).find(t=>t.source.startsWith(`${group}:`)&&t.status==='running'))
  await active(groupTask.targetConversation)
  await call('chat.correct',{conversation:group,task:groupTask.id,text:'CORRECT_TO group_latest'})
  await until('group corrected result',async()=>(await call('task.get',{id:groupTask.id})).status==='completed')
  const groupItems=await items(group)
  assert.ok(groupItems.some(i=>i.correction&&i.targetBot===worker.id))
  assert.equal(groupItems.filter(i=>i.kind==='bot'&&i.bot===worker.id&&i.text.includes('Corrected: group_latest')).length,1)
  await idle(groupTask.targetConversation)
  await until('prior group workflow delivered',async()=>(await call('workflow.list')).filter(w=>w.source.startsWith(group+':')).every(w=>['completed','needs_attention'].includes(w.status))&&!Object.keys((await state()).work.runs).some(k=>k.startsWith(group+':')))
  await call('chat.send',{conversation:group,text:'DELAY 8000\nordinary group current task'})
  const member=`${group}:${chief.id}`, groupRun=await active(member)
  await call('chat.correct',{conversation:group,run:member,since:groupRun.since,text:'CORRECT_TO group_member'})
  await until('normal group seat correction',async()=>(await items(group)).some(i=>i.kind==='bot'&&i.text.includes('Corrected: group_member')))
  console.log('ok   group delegation and normal group member turns accept targeted corrections without launching a second group round')
} finally {
  await app?.stop()
  console.log('ok   isolated correction test service cleaned up')
}
