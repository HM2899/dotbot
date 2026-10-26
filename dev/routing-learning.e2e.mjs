#!/usr/bin/env node
// Measured routing and staged learning use real terminal/file checks; the model is a fixture.
import assert from 'node:assert/strict'
import {readFileSync} from 'node:fs'
import {startServer,until} from './harness.mjs'
let app
try {
  app=await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0',DOTBOT_LEARNING_DELAY_MS:'300'}})
  const {call}=app,state=()=>call('team.state')
  await call('settings.learning',{enabled:false});await call('settings.local-computer',{enabled:true})
  const chief=(await state()).bots[0],one=await call('bot.create',{name:'核查 A',role:'Review',model:'review-one'}),two=await call('bot.create',{name:'核查 B',role:'Review',model:'review-two'})
  const settle=id=>until('flow settles',async()=>{for(const a of (await state()).work.approvals)await call('approval.answer',{id:a.id,allow:true});return(await call('workflow.list')).find(w=>w.id===id&&['completed','needs_attention'].includes(w.status))},40000)
  const idle=()=>until('idle',async()=>!Object.keys((await state()).work.runs).length,20000)
  await call('chat.send',{conversation:`bot:${one.id}`,text:'DELAY 6000\nnormal foreground work'})
  await until('foreground load counted',async()=>(await state()).bots.find(b=>b.id===one.id).performance.running>=1)
  await call('chat.stop',{conversation:`bot:${one.id}`});await idle()
  await call('skill.save',{name:'records',description:'校验电子表导出条数',text:'先比较输入与输出数量，保存 JSON，再检查实际文件。'})
  const command=`node -e 'const fs=require("fs");let rows=[1,2,3];if(rows.length!==3)process.exit(1);fs.writeFileSync("checked.json",JSON.stringify({count:rows.length}));'`
  const goal=`TOOL local_run ${JSON.stringify({command})}\nTOOL local_send_file ${JSON.stringify({path:'__TASK_CWD__/checked.json'})}`
  let flow=await call('skill.trial',{name:'records',bot:two.id,goal,acceptance:'文件必须是有效 JSON，并且 count 为 3。'})
  assert.equal((await settle(flow)).status,'completed');await idle()
  let before=await state(),stats=before.bots.find(b=>b.id===two.id).performance
  assert.equal(stats.verifiedPasses,1)
  await call('chat.send',{conversation:chief.conversation,text:`请核对电子表导出数量 AUTO_COORDINATE measured PERFORMANCE_ROUTING\n${goal}`})
  const allocated=await until('measured assignment',async()=>(await call('task.list')).find(t=>t.source===chief.conversation&&t.assignment?.objective.includes('AUTO_RESULT measured')))
  assert.equal(allocated.target,two.id);assert.ok(allocated.routing);await until('ordinary task completes',async()=>(await call('task.get',{id:allocated.id})).status==='completed');await idle()
  const logs=readFileSync(app.log,'utf8').trim().split('\n').map(JSON.parse)
  assert.ok(logs.some(l=>l.args.includes('--model')&&l.args.includes('review-two')))
  await call('bot.update',{id:two.id,model:'new-model'})
  assert.equal((await state()).bots.find(b=>b.id===two.id).performance.samples,0,'old-model evidence must not be current expertise')
  console.log('ok   actual skill-test evidence and duration reach ordinary allocation and receipts; model changes invalidate current-setting samples')

  await call('settings.learning',{enabled:true})
  const reviewRequest=`AUTO_LEARN_SUCCESS candidateproof\nTOOL read_skill {"name":"records"}\nTOOL local_run ${JSON.stringify({command:'printf "source=3 output=3 verified"'})}`
  await call('chat.send',{conversation:chief.conversation,text:reviewRequest})
  const review=await until('candidate reflection',async()=>{for(const a of (await state()).work.approvals)await call('approval.answer',{id:a.id,allow:true});return(await call('learning.list',{bot:chief.id})).find(r=>r.candidate)},25000)
  const active=await call('skill.get',{name:'records'})
  assert.ok(!active.body.includes('candidateproof'))
  await call('skill.benchmark.save',{name:'records',goal:'REGRESSION_FAIL_CANDIDATE',acceptance:'产物必须是可解析的 JSON。'})
  await idle();await call('skill.candidate.test',{id:review.candidate})
  const failed=await until('bad candidate blocked',async()=>{for(const a of (await state()).work.approvals)await call('approval.answer',{id:a.id,allow:true});return(await call('skill.candidates',{name:'records'})).find(c=>c.id===review.candidate&&c.status==='failed')},40000)
  assert.ok(failed.runs.some(r=>r.phase==='regbase'&&r.passed));assert.ok(failed.runs.some(r=>r.phase==='regcandidate'&&!r.passed))
  assert.equal((await call('skill.get',{name:'records'})).revision,active.revision)
  const jobs=(await call('task.list')).filter(t=>t.request?.includes(review.candidate));assert.equal(jobs.length,4)
  assert.ok(jobs.every(t=>!Object.hasOwn(t,'maxBudgetUsd')&&t.executionProfile.model===(chief.model??'fake-model')))
  const all=readFileSync(app.log,'utf8').trim().split('\n').map(JSON.parse)
  assert.ok(all.some(l=>l.args.some(a=>a.endsWith('/baseline'))));assert.ok(all.some(l=>l.args.some(a=>a.endsWith('/candidate'))))
  console.log('ok   baseline/candidate load isolated frozen plugins; corrupt candidate fails real parsing and cannot replace the active skill; all phases retain original Bot settings without budget options')
} finally {await app?.stop()}
