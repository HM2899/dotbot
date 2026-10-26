#!/usr/bin/env node
// Removed scheduling/reporting features stay absent; ordinary replies still work.
import assert from 'node:assert/strict'
import {readFileSync,existsSync} from 'node:fs'
import {join} from 'node:path'
import {startServer,until} from './harness.mjs'
let app
try{
 app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 const {call}=app;await call('settings.learning',{enabled:false})
 const chief=(await call('team.state')).bots[0],worker=await call('bot.create',{name:'Writer',role:'Writes',model:'kept-model'})
 const items=conversation=>call('chat.page',{conversation}).then(p=>p.items)
 const idle=()=>until('idle',async()=>!Object.keys((await call('team.state')).work.runs).length)
 const lifecycle=()=>existsSync(app.log+'.turns')?readFileSync(app.log+'.turns','utf8').trim().split('\n').map(line=>JSON.parse(line)):[]
 const start=marker=>lifecycle().find(row=>row.event==='start'&&row.prompt.includes(marker))
 const end=pid=>lifecycle().find(row=>row.event==='end'&&row.pid===pid)
 await call('chat.send',{conversation:`bot:${worker.id}`,text:'DELAY 4000\nfirst concurrent request'})
 const first=await until('first model starts',()=>start('first concurrent request'))
 await call('chat.send',{conversation:chief.conversation,text:'DELAY 2000\nsecond concurrent request'})
 const second=await until('other Bot starts before the first completes',()=>start('second concurrent request'))
 assert.ok(!end(first.pid),'different Bots must execute concurrently')
 await idle()
 assert.ok(end(first.pid));assert.ok(end(second.pid))
 assert.ok((await items(`bot:${worker.id}`)).some(i=>i.text==='Echo: first concurrent request'))
 assert.ok((await items(chief.conversation)).some(i=>i.text==='Echo: second concurrent request'))
 await call('chat.send',{conversation:`bot:${worker.id}`,text:'DELAY 1500\nsame bot first request'})
 const same=await until('same Bot first model starts',()=>start('same bot first request'))
 await call('chat.send',{conversation:`bot:${worker.id}`,text:'same bot second request'})
 await until('same Bot second queued',async()=>((await call('team.state')).work.runs[`bot:${worker.id}`]?.queued||0)===1)
 assert.equal(Boolean(start('same bot second request')),false)
 const next=await until('same Bot second starts',()=>start('same bot second request'))
 assert.ok(end(same.pid));assert.ok(next.at>=end(same.pid).at)
 await idle()
 console.log('ok   different Bots execute concurrently; each Bot keeps one shared execution slot')
 const upload=await fetch(`${app.base}/api/files?${new URLSearchParams({conversation:chief.conversation,name:'evidence.txt'})}`,{method:'POST',headers:{cookie:app.cookie,'content-type':'application/octet-stream'},body:'original-evidence'})
 const file=(await upload.json()).file
 await call('chat.send',{conversation:chief.conversation,text:'TOOL message_bot '+JSON.stringify({bot:'Writer',text:'REPORT_ATTACHMENT original-evidence'}),files:[file.src]})
 const task=await until('delegated reply returned',async()=>{const task=(await call('task.list')).find(t=>t.text==='REPORT_ATTACHMENT original-evidence'&&t.status==='completed');return task&&(await items(chief.conversation)).some(i=>i.kind==='incoming'&&i.text.includes('text=true'))&&task})
 assert.equal(task.workflow,null);assert.deepEqual(task.inputs,[file.src]);assert.equal('spentUsd' in task,false);assert.equal('maxBudgetUsd' in task,false)
 assert.equal((await call('team.state')).bots.find(b=>b.id===worker.id).model,'kept-model')
 await idle();assert.equal((await call('workflow.list')).length,0)
 assert.ok((await items(chief.conversation)).every(i=>i.kind!=='delivery'))
 console.log('ok   ordinary delegation returns messages and inherited attachments with original Bot settings')
 await call('chat.send',{conversation:chief.conversation,text:'TOOL message_bot {"bot":"Writer","text":"FAIL deliberate failure"}'})
 await until('failure returned normally',async()=>(await items(chief.conversation)).some(i=>i.kind==='incoming'&&/Task failed: .*deliberate failure/.test(i.text)))
 await idle()
 const room=await call('room.create',{name:'普通协作',members:[chief.id,worker.id],admin:chief.id})
 await call('chat.send',{conversation:`room:${room.id}`,text:'@Writer group normal reply'})
 await until('group reply',async()=>(await items(`room:${room.id}`)).some(i=>i.who==='Writer'&&i.text.includes('group normal reply')))
 await idle()
 for(const method of ['usage.view','settings.execution','task.pause','task.resume','task.priority','workflow.retry'])await assert.rejects(call(method,{id:task.id,taskBudgetUsd:1}),/Unknown method/)
 const runs=readFileSync(app.log,'utf8').trim().split('\n').map(s=>JSON.parse(s).args)
 assert.ok(runs.every(args=>!args.includes('--max-budget-usd')))
 assert.equal(existsSync(join(app.home,'team/usage.jsonl')),false)
 const state=await call('team.state');assert.equal('executionLimits' in state.work,false);assert.ok(!JSON.stringify(state.bots).includes('medianCostUsd'))
 console.log('ok   failed work and group chat still reply; removed APIs, charges, limits and report cards stay absent')
}finally{await app?.stop()}
