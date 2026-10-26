#!/usr/bin/env node
// Real completed task receipts, terminal checks, reflection persistence and reuse.
// Models are fixtures; no external accounts or paid inference.
import assert from 'node:assert/strict'
import { readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until } from './harness.mjs'
let app
try {
  const extraEnv={DOTBOT_LEARNING_DELAY_MS:'300',DOTBOT_SKILL_DEFAULTS:'0'}
  app=await startServer({extraEnv,keepHome:true})
  let call=app.call
  const state=()=>call('team.state'), reviews=bot=>call('learning.list',{bot}), items=conversation=>call('chat.page',{conversation}).then(x=>x.items)
  const idle=conversation=>until('idle',async()=>!(await state()).work.runs[conversation])
  const run=async(conversation,text)=>{
    await idle(conversation);const before=new Set((await call('recovery.list')).map(w=>w.id))
    await call('chat.send',{conversation,text})
    const work=await until('work receipt',async()=>(await call('recovery.list')).find(w=>!before.has(w.id)&&w.conversation===conversation))
    const done=await until('task result',async()=>{
      for(const approval of (await state()).work.approvals)await call('approval.answer',{id:approval.id,allow:true})
      const w=await call('recovery.get',{id:work.id});return ['completed','failed'].includes(w.status)?w:null
    })
    await idle(conversation);return done
  }
  // Separate background reviews are opt-in; this suite covers that mode.
  assert.notEqual((await state()).prefs.learningEnabled,true,'background reviews are off by default')
  await call('settings.learning',{enabled:true})
  const chief=(await state()).bots[0]
  await call('bot.update',{id:chief.id,model:'learning-model',permission:'ask'})
  await call('settings.local-computer',{enabled:true})
  await call('skill.save',{name:'records',description:'Validate exported records',text:'Inspect source records, generate the output, compare counts and deliver the artifact with evidence.'})
  const request=token=>`AUTO_LEARN_${token}\nTOOL read_skill {"name":"records"}\nTOOL local_run {"command":"printf 'source=3 output=3 verified'","timeout_s":5}`
  const task=await run(chief.conversation,request('SUCCESS records'))
  const learned=await until('candidate staged',async()=>(await reviews(chief.id)).find(r=>r.work===task.id&&r.skillState==='candidate'))
  assert.equal(learned.status,'reviewed');assert.equal(learned.outcome,'completed')
  assert.ok(learned.lessons[0].evidence.some(id=>task.actions.some(a=>a.id===id&&a.tool==='local_run'&&a.state==='confirmed')))
  let skill=await call('skill.get',{name:'records'})
  assert.ok(!skill.body.includes('verify output record counts'),'unverified candidate must not change the active skill')
  const caseGoal='TOOL local_run '+JSON.stringify({command:'node -e \'const fs=require("fs");const source=[1,2,3],output=[1,2,3];if(source.length!==output.length)process.exit(1);fs.writeFileSync("checked.json",JSON.stringify({count:output.length}));\''})+'\nTOOL local_send_file '+JSON.stringify({path:'__TASK_CWD__/checked.json'})
  await call('skill.benchmark.save',{name:'records',goal:caseGoal,acceptance:'独立目录生成有效 JSON，实际数量必须等于 3。'})
  await call('skill.candidate.test',{id:learned.candidate})
  await until('paired candidate promoted',async()=>{
    for(const a of (await state()).work.approvals)await call('approval.answer',{id:a.id,allow:true})
    const c=(await call('skill.candidates',{name:'records'})).find(c=>c.id===learned.candidate)
    if(['failed','conflict'].includes(c.status)){const tasks=(await call('task.list')).filter(t=>t.request?.includes(c.id));const works=await Promise.all(tasks.map(async t=>({task:t.id,verification:t.verification,work:(await call('recovery.list')).find(w=>w.task===t.id)})));throw new Error(JSON.stringify({candidate:c,works}))}
    return c.status==='promoted'
  },40000)
  skill=await call('skill.get',{name:'records'})
  assert.match(skill.body,/verify output record counts/)
  assert.equal(skill.body.split(`teambot-learning:${learned.id}`).length-1,1)
  const applied=await run(chief.conversation,'检查 records validation USE_EXPERIENCE records')
  assert.equal(applied.result.text,'Applied experience: records')
  const helper=await call('bot.create',{name:'Observer',role:'Review evidence',model:'observer-model'})
  const separate=await run(`bot:${helper.id}`,'检查 records validation USE_EXPERIENCE records')
  assert.equal(separate.result.text,'No relevant experience')
  console.log('ok   candidate leaves active skill untouched until paired real file checks pass; learned methods are used on the next relevant task and remain Bot-scoped')

  const failed=await run(chief.conversation,'FAIL source-unavailable\nAUTO_LEARN_FAILURE failures')
  const failureReview=await until('failure reviewed',async()=>(await reviews(chief.id)).find(r=>r.work===failed.id&&r.status==='reviewed'))
  assert.equal(failureReview.outcome,'failed');assert.equal(failureReview.skillUpdate,null)
  assert.deepEqual(failureReview.lessons[0].evidence,['result'])
  const invalid=await run(chief.conversation,'分析记录 AUTO_LEARN_INVALID invalid')
  const rejected=await until('invalid review rejected',async()=>(await reviews(chief.id)).find(r=>r.work===invalid.id&&r.status==='failed'))
  assert.equal(rejected.lessons.length,0);assert.equal(rejected.skillUpdate,null)
  await call('chat.send',{conversation:chief.conversation,text:'DELAY 6000\nAUTO_LEARN_SUCCESS cancelled'})
  const cancelled=await until('cancellable work',async()=>(await call('recovery.list')).find(w=>w.text.includes('AUTO_LEARN_SUCCESS cancelled')))
  await call('chat.stop',{conversation:chief.conversation});await idle(chief.conversation)
  assert.equal((await reviews(chief.id)).some(r=>r.work===cancelled.id),false)
  console.log('ok   failed tasks retain failure status and self-report limits; invented evidence and cancelled tasks cannot teach verified methods or change skills')

  const room=await call('room.create',{name:'Learning group',members:[chief.id,helper.id],admin:chief.id})
  const group=`room:${room.id}`
  await call('chat.send',{conversation:group,text:'分析资料 AUTO_LEARN_SUCCESS grouprecords'})
  const groupReview=await until('group retrospective',async()=>(await reviews(chief.id)).find(r=>r.conversation===group&&r.summary.includes('grouprecords')&&r.status==='reviewed'))
  assert.equal(groupReview.skillUpdate,null)
  await call('chat.send',{conversation:group,text:'检查 grouprecords validation USE_EXPERIENCE grouprecords'})
  await until('group experience reused',async()=>(await items(group)).some(i=>i.kind==='bot'&&i.text==='Applied experience: grouprecords'))
  await until('group idle',async()=>!Object.keys((await state()).work.runs).some(k=>k.startsWith(`${group}:`)))
  assert.equal((await run(chief.conversation,'检查 grouprecords validation USE_EXPERIENCE grouprecords')).result.text,'No relevant experience')
  console.log('ok   completed group rounds are reviewed and lessons reused in the group without leaking into private chats')

  const conflictTask=await run(chief.conversation,request('SLOW conflict'))
  await until('slow refinement reviewing',async()=>(await reviews(chief.id)).some(r=>r.work===conflictTask.id&&r.status==='reviewing'))
  skill=await call('skill.get',{name:'records'})
  await call('skill.save',{name:'records',description:skill.description,text:'User revision must survive concurrent retrospective.',revision:skill.revision})
  const conflict=await until('concurrent skill edit preserved',async()=>(await reviews(chief.id)).find(r=>r.work===conflictTask.id&&r.skillState==='conflict'))
  assert.ok(conflict.error);assert.equal((await call('skill.get',{name:'records'})).body,'User revision must survive concurrent retrospective.')
  const slow=await run(chief.conversation,'分析内容 AUTO_LEARN_SLOW preemption')
  await until('idle review active',async()=>(await reviews(chief.id)).some(r=>r.work===slow.id&&r.status==='reviewing'))
  const started=Date.now();await run(chief.conversation,'你好')
  assert.ok(Date.now()-started<4000,'foreground chat must not wait for a slow retrospective')
  await until('background deferred',async()=>(await reviews(chief.id)).some(r=>r.work===slow.id&&r.status==='deferred'))
  console.log('ok   foreground work interrupts tool-free reviews; concurrent user skill edits win over automatic refinements')

  await call('settings.learning',{enabled:false})
  const old=await run(chief.conversation,'分析历史资料 AUTO_LEARN_SUCCESS historical')
  assert.equal((await reviews(chief.id)).some(r=>r.work===old.id),false)
  await call('learning.forget',{id:learned.id})
  assert.equal((await reviews(chief.id)).find(r=>r.id===learned.id).status,'forgotten')
  const dir=app.home;await app.stop();app=await startServer({directory:dir,extraEnv,keepHome:true});call=app.call
  assert.equal((await state()).prefs.learningEnabled,false)
  assert.equal((await reviews(chief.id)).find(r=>r.id===learned.id).status,'forgotten')
  assert.equal((await run(chief.conversation,'检查 records validation USE_EXPERIENCE records')).result.text,'No relevant experience')
  await call('settings.learning',{enabled:true})
  await until('historical task backfilled',async()=>(await reviews(chief.id)).some(r=>r.work===old.id&&r.status==='reviewed'))
  assert.equal((await reviews(chief.id)).filter(r=>r.work===old.id).length,1)
  const logs=readFileSync(app.log,'utf8').trim().split('\n').map(s=>JSON.parse(s).args)
  const reflection=logs.filter(a=>a.includes('--json-schema')&&JSON.parse(a[a.indexOf('--json-schema')+1]).properties.reflection)
  assert.ok(reflection.length);assert.ok(reflection.some(a=>a[a.indexOf('--model')+1]==='learning-model'))
  assert.ok(reflection.every(a=>!a.includes('--mcp-config')&&a[a.indexOf('--tools')+1]===''))
  assert.ok(!readFileSync(join(app.home,'team/learning.json'),'utf8').includes('invented-evidence'))
  console.log('ok   retrospectives/settings survive restart; historical tasks backfill once, forgotten lessons stay forgotten and reviews use Bot models without execution tools')
}finally{if(app){const dir=app.home;await app.stop();rmSync(dir,{recursive:true,force:true})}}
