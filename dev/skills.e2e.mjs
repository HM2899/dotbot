#!/usr/bin/env node
// Real skill preparation, storage, source downloads and tool boundaries; model responses are fixtures.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFileSync, existsSync, readdirSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until } from './harness.mjs'
const bundles = {
  pdf: {
    'SKILL.md': '---\nname: pdf\ndescription: >-\n  Extract PDF content with\n  page-level checks.\n---\n\nRead references/checks.md, inspect scripts/extract.py, then extract and verify page coverage. AUTO_SKILL_WORKFLOW pdf',
    'references/checks.md': 'Compare source and output page counts; retain page references.',
    'scripts/extract.py': 'raise RuntimeError("must not execute at install")',
    'assets/sample.bin': Buffer.from([0,255,128,5])
  }
}
let unavailable=false, brokenBlob=false, requests=0
const blobs=new Map(), tree=[]
for (const [name,files] of Object.entries(bundles)) for (const [path,content] of Object.entries(files)) {
  const bytes=Buffer.from(content), sha=String(blobs.size+1).padStart(40,'0')
  tree.push({path:`skills/${name}/${path}`,type:'blob',mode:'100644',sha,size:bytes.length}); blobs.set(sha,bytes)
}
const licenseSha='f'.repeat(40); blobs.set(licenseSha,Buffer.from('Upstream license notice'))
tree.push({path:'LICENSE',type:'blob',mode:'100644',sha:licenseSha,size:23})
const source=createServer((req,res)=>{
  requests++
  res.setHeader('content-type','application/json')
  if(unavailable){res.writeHead(503);res.end('{}');return}
  if(brokenBlob && req.url.includes('/git/blobs/') && req.url.endsWith('2')){res.writeHead(503);res.end('{}');return}
  if(req.url==='/repos/anthropics/skills/git/trees/main?recursive=1') res.end(JSON.stringify({sha:'a'.repeat(40),truncated:false,tree}))
  else {const sha=req.url.split('/').at(-1),bytes=blobs.get(sha);if(!bytes){res.writeHead(404);res.end('{}')}else res.end(JSON.stringify({sha,encoding:'base64',content:bytes.toString('base64')}))}
})
await new Promise(resolve=>source.listen(0,'127.0.0.1',resolve))
let app
const env={DOTBOT_SKILL_API_BASE:`http://127.0.0.1:${source.address().port}`}
try {
  app=await startServer({extraEnv:env,keepHome:true})
  let {call}=app
  const state=()=>call('team.state'), items=conversation=>call('chat.page',{conversation}).then(x=>x.items)
  const idle=conversation=>until('idle',async()=>!(await state()).work.runs[conversation])
  const run=async(conversation,text)=>{
    await idle(conversation);const before=(await items(conversation)).at(-1)?.id
    await call('chat.send',{conversation,text})
    const answer=await until('skill task reply',async()=>{const all=await items(conversation),i=before?all.findIndex(x=>x.id===before):-1;return all.slice(i+1).find(x=>x.kind==='bot')})
    await idle(conversation);return answer
  }
  const chief=(await state()).bots[0]
  const reviewer=await call('bot.create',{name:'Researcher',role:'Review evidence and source provenance',model:'review-model'})
  const writer=await call('bot.create',{name:'Writer',role:'Write and revise clear copy',model:'writer-model'})
  reviewer.conversation=`bot:${reviewer.id}`;writer.conversation=`bot:${writer.id}`
  const a=await run(reviewer.conversation,'分析资料 AUTO_SKILL_CREATE evidence')
  const skill=(await state()).skills.find(s=>s.origin.by===reviewer.id)
  assert.ok(skill);assert.match(a.text,new RegExp(skill.name));assert.equal(skill.origin.kind,'authored');assert.equal(skill.origin.validation,'untested')
  assert.match(skill.description,/Review evidence/)
  await run(reviewer.conversation,'分析资料 AUTO_SKILL_CREATE evidence')
  assert.equal((await state()).skills.length,1)
  const profile=(await state()).bots.find(b=>b.id===reviewer.id).skillProfile
  assert.equal(profile[0].loads,2);assert.equal(profile[0].lastOutcome,'turn-completed')
  await run(writer.conversation,'编写说明 AUTO_SKILL_CREATE copy')
  assert.equal((await state()).bots.find(b=>b.id===writer.id).skillProfile.length,1)
  assert.notEqual((await state()).bots.find(b=>b.id===writer.id).skillProfile[0].name,skill.name)
  await call('bot.update',{id:reviewer.id,skills:[skill.name,skill.name]})
  assert.deepEqual((await state()).bots.find(b=>b.id===reviewer.id).skills,[skill.name])
  await assert.rejects(call('bot.update',{id:reviewer.id,skills:['missing']}))
  console.log('ok   individual role-specific workflows are created, applied in the same turn, reused and preferred per Bot')

  brokenBlob=true
  await assert.rejects(call('skill.install',{source:'anthropic/pdf'}))
  assert.equal(existsSync(join(app.home,'team/plugin/skills/pdf')),false)
  assert.equal((await state()).skills.some(s=>s.name==='pdf'),false)
  brokenBlob=false
  const imported=await run(reviewer.conversation,'提取 PDF 内容 AUTO_SKILL_INSTALL pdf')
  assert.match(imported.text,/pdf/)
  const root=join(app.home,'team/plugin/skills/pdf')
  for(const path of Object.keys(bundles.pdf)) assert.ok(existsSync(join(root,path)),path)
  assert.equal(readFileSync(join(root,'UPSTREAM-LICENSE'),'utf8'),'Upstream license notice')
  const pdf=await call('skill.get',{name:'pdf'})
  assert.equal(pdf.origin.tree,'a'.repeat(40));assert.equal(pdf.origin.source,'anthropic/pdf');assert.equal(pdf.origin.validation,'untested')
  const count=requests;await call('skill.install',{source:'anthropic/pdf'});assert.equal(requests,count)
  await assert.rejects(call('skill.install',{source:'https://example.com/evil'}))
  const reference=await run(reviewer.conversation,'TOOL read_skill_resource {"name":"pdf","path":"references/checks.md"}')
  assert.match(reference.text,/source and output page counts/)
  const binary=await run(reviewer.conversation,'TOOL read_skill_resource {"name":"pdf","path":"assets/sample.bin"}')
  assert.match(binary.text,/media\//)
  const traversal=await run(reviewer.conversation,'TOOL read_skill_resource {"name":"pdf","path":"../secret"}')
  assert.match(traversal.text,/Invalid skill resource path/)
  const discover=await run(reviewer.conversation,'TOOL find_skills {"query":"PDF"}')
  assert.match(discover.text,/anthropic\/pdf/)
  console.log('ok   source imports preserve scripts/references/assets/licenses and provenance, support resource reads and reject traversal')

  unavailable=true
  const fallback=await run(writer.conversation,'分析表格 AUTO_SKILL_INSTALL xlsx')
  const draft=(await state()).skills.find(s=>s.name.startsWith('fallback-xlsx'))
  assert.ok(draft);assert.match(fallback.text,new RegExp(draft.name));assert.equal(draft.origin.kind,'authored')
  await call('skill.toggle',{name:'pdf',enabled:false,revision:pdf.revision})
  const before=(await state()).skills.length
  const disabled=await run(reviewer.conversation,'提取 PDF AUTO_SKILL_INSTALL pdf')
  assert.equal(disabled.text,'Skills applied: none');assert.equal((await call('skill.get',{name:'pdf'})).enabled,false)
  assert.equal((await state()).skills.length,before)
  await run(reviewer.conversation,'分析内容 AUTO_SKILL_INVALID bad');assert.equal((await state()).skills.length,before)
  await call('bot.update',{id:writer.id,permission:'readonly'})
  await run(writer.conversation,'分析内容 AUTO_SKILL_CREATE readonly');assert.equal((await state()).skills.length,before)
  const forbidden=await run(writer.conversation,'TOOL install_skill {"source":"anthropic/docx"}')
  assert.match(forbidden.text,/read.only/i)
  await call('bot.update',{id:writer.id,permission:'auto'})
  await call('chat.send',{conversation:writer.conversation,text:'分析内容 AUTO_SKILL_SLOW cancelled'})
  await until('preparation active',async()=>(await state()).work.runs[writer.conversation]?.activity==='preparing-skills')
  await call('chat.stop',{conversation:writer.conversation});await idle(writer.conversation)
  assert.equal((await state()).skills.length,before)
  console.log('ok   unavailable sources fall back to useful drafts; disabled/invalid/readonly/stopped plans cannot install or overwrite skills')

  const group=await call('room.create',{name:'Skills group',members:[chief.id,reviewer.id],admin:chief.id})
  await call('chat.send',{conversation:`room:${group.id}`,text:'@Researcher 分析资料 AUTO_SKILL_CREATE evidence'})
  await until('group applies specialist workflow',async()=>(await items(`room:${group.id}`)).some(i=>i.kind==='bot'&&i.text.includes(skill.name)))
  await until('group idle',async()=>!Object.keys((await state()).work.runs).some(k=>k.startsWith(`room:${group.id}:`)))
  await call('chat.send',{conversation:chief.conversation,text:`核查并汇总 AUTO_COORDINATE specialty_routing SKILL_ROUTING ${draft.name}`})
  const routed=await until('skill-aware allocation',async()=>(await call('task.list')).find(t=>t.automatic&&t.text.includes('specialty_routing')))
  assert.equal(routed.target,writer.id)
  await until('skill-aware integration',async()=>(await items(chief.conversation)).some(i=>i.kind==='bot'&&i.text==='Integrated: specialty_routing'))
  await idle(chief.conversation)
  const logs=readFileSync(app.log,'utf8').trim().split('\n').map(x=>JSON.parse(x).args)
  const plans=logs.filter(a=>a.includes('--json-schema')&&JSON.parse(a[a.indexOf('--json-schema')+1]).properties.skillPlan)
  assert.ok(plans.length&&plans.every(a=>a[a.indexOf('--model')+1]==='fast-model'),'skill selection runs on the planning model')
  assert.ok(plans.every(a=>!a.includes('--mcp-config')),'preparation is tool-free')
  const dir=app.home;await app.stop();app=await startServer({directory:dir,keepHome:true,extraEnv:env});call=app.call
  assert.equal((await call('skill.get',{name:'pdf'})).enabled,false)
  const restored=(await state()).bots.find(b=>b.id===reviewer.id)
  assert.deepEqual(restored.skills,[skill.name]);assert.ok(restored.skillProfile.find(s=>s.name===skill.name).loads>=3)
  assert.equal(readdirSync(join(dir,'team/skill-staging')).length,0)
  console.log('ok   group seats reuse specialties, Main allocation receives skill profiles, preserves configured models, and skill preferences/profiles survive restart')
} finally {
  if(app){const dir=app.home;await app.stop();rmSync(dir,{recursive:true,force:true})}
  await new Promise(resolve=>source.close(resolve))
}
