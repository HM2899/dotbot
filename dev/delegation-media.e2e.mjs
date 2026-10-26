#!/usr/bin/env node
// Real scheduler, persistence, SSE and image HTTP calls with local model/provider fixtures.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { appendFileSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { startServer, until } from './harness.mjs'

let app, feedTask, failImage = false, failVideo = false
const feedController = new AbortController(), events = [], imageRequests = [], videoRequests = []
const privatePrompt = 'PRIVATE_IMAGE_PROMPT_7e81 lighting composition negative-prompt internal-render-settings'
const privateVideoPrompt = 'PRIVATE_VIDEO_PROMPT_91d2 motion camera trajectory negative-prompt internal-render-settings'
const imageServer = createServer(async (req, res) => {
  if (req.url === '/fixture.mp4') {
    res.setHeader('content-type','video/mp4'); res.end(Buffer.from('00000018667479706d703432000000006d70343269736f6d','hex')); return
  }
  if (req.url === '/async-result/video-fixture') {
    res.setHeader('content-type','application/json')
    res.end(JSON.stringify({task_status:'SUCCESS',video_result:[{url:`http://127.0.0.1:${imageServer.address().port}/fixture.mp4`}]})); return
  }
  let raw = ''; for await (const chunk of req) raw += chunk
  const body = JSON.parse(raw)
  if (req.url === '/videos/generations') {
    videoRequests.push(body); res.setHeader('content-type','application/json')
    if (failVideo) { res.statusCode=400; res.end(JSON.stringify({error:{message:`rejected video prompt: ${body.prompt}`}})) }
    else res.end(JSON.stringify({id:'video-fixture'}))
    return
  }
  imageRequests.push(body)
  res.setHeader('content-type', 'application/json')
  if (failImage) { res.statusCode = 400; res.end(JSON.stringify({error:{message:`rejected prompt: ${body.prompt}`}})) }
  else res.end(JSON.stringify({data:[{b64_json:'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII='}]}))
})
try {
  await new Promise(resolve => imageServer.listen(0, '127.0.0.1', resolve))
  app = await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined})
  const { call } = app
  await call('settings.learning', { enabled:false })
  const chief = (await call('team.state')).bots[0]
  const preferred = await call('skill.save',{name:'delegation-method',description:'Evidence validation workflow',text:'Check original sources, perform the bounded work and return verified results.'})
  await call('bot.update', { id:chief.id, model:'delegation-model', effort:'high', permission:'edits',workDir:join(app.home,'work'),skills:[preferred] })
  const veteran = await call('bot.create', { name:'ExistingReviewer', role:'Review evidence', model:'existing-model' })
  await call('bot.update', { id:veteran.id, permission:'ask', effort:'medium' })
  const items = async conversation => (await call('chat.page', {conversation})).items
  const idle = conversation => until('request idle', async()=> !(await call('team.state')).work.runs[conversation])
  const delegate = async (conversation, parent, token, extra = {}) => {
    const botsBefore = (await call('team.state')).bots
    const before = new Set(botsBefore.map(b=>b.id))
    const expected = extra.reuse
    const {reuse, fresh, force, ...message} = extra
    const priorTasks = new Set((await call('task.list')).map(t=>t.id)), start = (await items(conversation)).length
    await call('chat.send', {conversation, text:`请委派完成资料核查 AUTO_RESULT ${token}\nDELAY 500${expected && force ? `\nREUSE_BOT ${expected}` : fresh ? '\nNO_MATCH' : ''}`, ...message})
    const task = await until('delegated Bot task', async()=> (await call('task.list')).find(t=>!priorTasks.has(t.id)&&t.text.includes(token)&&t.automatic))
    if (expected) assert.equal(task.target, expected, 'a suitable existing Bot must be reused')
    else assert.ok(!before.has(task.target), 'create a worker only when no candidate is suitable')
    const state = await call('team.state'), worker = state.bots.find(b=>b.id===task.target), origin = state.bots.find(b=>b.id===parent.id)
    assert.equal(state.bots.length, before.size+(expected ? 0 : 1))
    if (expected) {
      assert.deepEqual(worker, botsBefore.find(b=>b.id===expected), 'reuse must preserve all existing Bot settings')
    } else {
      assert.equal(worker.model, origin.model)
      assert.equal(worker.permission, origin.permission)
      assert.equal(worker.effort, origin.effort)
      assert.deepEqual(worker.skills??[],origin.skills??[])
      const effectiveDir=state.rooms.find(r=>r.conversation===conversation)?.workDir??origin.workDir
      assert.equal(worker.workDir,effectiveDir)
      assert.equal(worker.createdBy, parent.id)
    }
    await until('result integrated in original chat', async()=> (await items(conversation)).slice(start).some(i=>i.kind==='bot'&&i.text===`Integrated: ${token}`))
    assert.equal((await call('task.get', {id:task.id})).status, 'completed')
    assert.equal((await call('task.list')).filter(t=>t.request===task.request).length,1,'workers and result callbacks must not create recursive delegations')
    assert.equal((await call('team.state')).bots.length,before.size+(expected ? 0 : 1))
    return task
  }
  await delegate(chief.conversation, chief, 'explicit_main', {reuse:veteran.id})
  await delegate(chief.conversation, chief, 'explicit_main', {reuse:veteran.id})
  const created = await delegate(`bot:${veteran.id}`, veteran, 'explicit_nonmain', {fresh:true})
  await delegate(`bot:${veteran.id}`, veteran, 'explicit_nonmain_again', {reuse:created.target,force:true})
  const room = await call('room.create',{name:'明确委派群',members:[chief.id,veteran.id],admin:veteran.id})
  const group = `room:${room.id}`
  await call('room.update',{id:room.id,workDir:join(app.home,'group-work')})
  for (const mode of ['auto','admin','all']) {
    await call('room.update',{id:room.id,mode})
    const task = await delegate(group,veteran,`explicit_group_${mode}`,{reuse:created.target,force:true})
    assert.equal(task.targetConversation.split('~task~')[0],`${group}:${task.target}`)
    assert.ok((await call('team.state')).rooms.find(r=>r.id===room.id).members.includes(task.target))
    assert.equal((await items(`bot:${task.target}`)).some(i=>i.text?.includes(`explicit_group_${mode}`)),false)
  }
  await delegate(group,veteran,'explicit_group_new_specialty',{fresh:true})
  await call('chat.send',{conversation:chief.conversation,text:'委派核查恢复逻辑\nFAIL explicit-recovery-fixture'})
  const failed=await until('failed explicit worker',async()=>(await call('task.list')).find(t=>t.text.includes('explicit-recovery-fixture')&&t.status==='failed'))
  await idle(chief.conversation)
  const beforeFailedResult=(await call('team.state')).bots.length
  await until('failed result returned',async()=>(await items(chief.conversation)).some(i=>i.kind==='bot'&&/Task failed: .*explicit-recovery-fixture/.test(i.text)))
  assert.equal((await call('team.state')).bots.length,beforeFailedResult,'a failed result callback must not create another Bot')
  assert.equal((await call('task.list')).filter(t=>t.request===failed.request).length,1)
  const beforeInvalid=(await call('team.state')).bots.length, beforeInvalidTasks=(await call('task.list')).length
  await call('chat.send',{conversation:chief.conversation,text:'请委派核查 DELEGATION_INVALID'})
  await idle(chief.conversation)
  assert.equal((await call('team.state')).bots.length,beforeInvalid,'invalid routing must not create a fallback Bot')
  assert.equal((await call('task.list')).length,beforeInvalidTasks)
  const readonly=await call('bot.create',{name:'ReadonlyObserver',role:'Observe only'})
  await call('bot.update',{id:readonly.id,permission:'readonly'})
  const beforeReadonly=(await call('team.state')).bots.length
  const beforeReadonlyTasks=(await call('task.list')).length
  await call('chat.send',{conversation:chief.conversation,text:`请委派写文件 REQUIRES_WRITE REUSE_BOT ${readonly.id}`})
  await idle(chief.conversation)
  assert.equal((await call('task.list')).length,beforeReadonlyTasks,'routing cannot assign writes to read-only Bots')
  await call('chat.send',{conversation:`bot:${readonly.id}`,text:'请委派检查资料'})
  await idle(`bot:${readonly.id}`)
  assert.equal((await call('team.state')).bots.length,beforeReadonly,'explicit requests do not bypass read-only policy')
  const total = (await call('team.state')).bots.length
  for (const text of ['不要委派，自己处理','解释委派是什么意思','引用："委派这个任务"','> 委派任务']) {
    await call('chat.send',{conversation:`bot:${veteran.id}`,text}); await idle(`bot:${veteran.id}`)
  }
  assert.equal((await call('team.state')).bots.length,total)
  await call('chat.send',{conversation:chief.conversation,text:'请委派核查 DELEGATION_SLOW'})
  await until('explicit selector active',async()=>(await call('team.state')).work.runs[chief.conversation]?.activity==='allocating')
  await call('chat.stop',{conversation:chief.conversation})
  await idle(chief.conversation)
  assert.equal((await call('team.state')).bots.length,total,'Stop during selection must not create a Bot')
  assert.equal((await call('task.list')).some(t=>t.text.includes('DELEGATION_SLOW')),false)
  await call('chat.send',{conversation:chief.conversation,text:'委派这项工作 AUTO_RESULT explicit_stop\nDELAY 6000'})
  const stopped = await until('new worker running',async()=>(await call('task.list')).find(t=>t.text.includes('explicit_stop')&&t.status==='running'))
  await call('chat.stop',{conversation:chief.conversation})
  await until('new worker cancelled',async()=>(await call('task.get',{id:stopped.id})).status==='cancelled')
  assert.equal((await items(chief.conversation)).some(i=>i.text==='Integrated: explicit_stop'),false)
  console.log('ok   explicit delegation reuses suitable Bots, creates only when needed in Main/non-Main chats and all group modes, retains settings, returns results and obeys Stop; negative/quoted requests do not create Bots')

  await idle(chief.conversation)
  const feed = await fetch(`${app.base}/api/bot/feed`,{headers:{cookie:app.cookie},signal:feedController.signal})
  feedTask = (async()=>{
    const reader=feed.body.getReader(),decoder=new TextDecoder();let buffer=''
    for (;;) {
      const {value,done}=await reader.read();if(done)return
      buffer+=decoder.decode(value,{stream:true})
      for(let end=buffer.indexOf('\n\n');end>=0;end=buffer.indexOf('\n\n')) {
        const block=buffer.slice(0,end);buffer=buffer.slice(end+2)
        const event=block.match(/^event: ?(.*)$/m)?.[1],data=block.match(/^data: ?(.*)$/m)?.[1]
        if(event&&data)events.push({event,data:JSON.parse(data)})
      }
    }
  })().catch(error=>{if(!feedController.signal.aborted)throw error})
  await call('provider.save',{kind:'image',name:'Local image fixture',baseUrl:`http://127.0.0.1:${imageServer.address().port}`,apiKey:'fixture-key',defaultModel:'fixture-image'})
  await call('chat.send',{conversation:chief.conversation,text:'请生图 IMAGE_PRIVACY_CASE'})
  await until('image and sanitized reply',async()=> (await items(chief.conversation)).some(i=>i.kind==='bot'&&i.text==='图片已生成。'))
  const generated=(await items(chief.conversation)).find(i=>i.kind==='file'&&i.generated==='image')
  assert.ok(generated)
  assert.equal(generated.caption,undefined)
  assert.equal(imageRequests[0].prompt,privatePrompt,'provider still receives the actual internal generation prompt')
  assert.equal((await fetch(`${app.base}/${generated.src}`,{headers:{cookie:app.cookie}})).status,200)
  assert.ok(events.some(e=>e.event==='work'&&Object.values(e.data.runs).some(r=>r.activity==='generating an image')))
  assert.equal(events.some(e=>e.event==='chat.stream'&&e.data.conversation===chief.conversation),false,'image drafts must not be streamed')
  const beforeGroupImages=imageRequests.length, beforeGroupBots=new Set((await call('team.state')).bots.map(b=>b.id))
  await call('chat.send',{conversation:group,text:'请委派生图 IMAGE_PRIVACY_CASE'})
  const imageTask=await until('delegated group image',async()=>(await call('task.list')).find(t=>t.source.startsWith(group+':')&&t.text.includes('IMAGE_PRIVACY_CASE')&&t.status==='completed'))
  assert.ok(!beforeGroupBots.has(imageTask.target))
  assert.equal(imageRequests.length,beforeGroupImages+1,'only the selected worker generates the delegated image')
  assert.equal(imageTask.files[0].caption,undefined)
  assert.equal((await items(group)).filter(i=>i.kind==='file'&&i.generated==='image').length,1,'the generated file returns to the group once')
  await until('group image synthesis finished',async()=>!Object.keys((await call('team.state')).work.runs).some(k=>k.startsWith(group+':')))
  assert.ok(!JSON.stringify(await items(group)).includes(privatePrompt))
  const beforeReuseImages=imageRequests.length, beforeReuseBots=(await call('team.state')).bots.length
  await call('chat.send',{conversation:group,text:`请委派生图 IMAGE_PRIVACY_CASE REUSE_BOT ${imageTask.target}`})
  await until('reused image worker',async()=>(await call('task.list')).some(t=>t.id!==imageTask.id&&t.target===imageTask.target&&t.text.includes('IMAGE_PRIVACY_CASE')&&t.status==='completed'))
  await until('reused image synthesis finished',async()=>!Object.keys((await call('team.state')).work.runs).some(k=>k.startsWith(group+':')))
  assert.equal(imageRequests.length,beforeReuseImages+1)
  assert.equal((await call('team.state')).bots.length,beforeReuseBots)
  assert.ok(!JSON.stringify(await items(group)).includes(privatePrompt))
  failImage=true
  await idle(chief.conversation)
  await call('chat.send',{conversation:chief.conversation,text:'请生图 IMAGE_PRIVACY_CASE_FAIL'})
  await until('sanitized provider failure',async()=> (await items(chief.conversation)).some(i=>i.kind==='bot'&&i.text.startsWith('图片生成未完成')))
  const legacy={id:'legacy-generated-fixture',kind:'file',time:Date.now(),src:generated.src,name:'image-20261024-123456-ab12.png',mime:'image/png',caption:privatePrompt}
  appendFileSync(join(app.home,'team','chats',chief.conversation.replaceAll(':','_')+'.jsonl'),JSON.stringify(legacy)+'\n')
  const shown=await items(chief.conversation)
  assert.equal(shown.find(i=>i.id===legacy.id).caption,undefined)
  assert.ok(!JSON.stringify(shown).includes(privatePrompt))
  assert.ok(!JSON.stringify(await call('team.state')).includes(privatePrompt))
  assert.ok(!JSON.stringify(events).includes(privatePrompt),'SSE must not carry raw prompts or reflected provider errors')
  await idle(chief.conversation)
  await call('provider.save',{kind:'video',name:'Local video fixture',baseUrl:`http://127.0.0.1:${imageServer.address().port}`,apiKey:'fixture-key',defaultModel:'fixture-video'})
  const videoEventsStart=events.length
  await call('chat.send',{conversation:chief.conversation,text:'请生成一段视频 VIDEO_PRIVACY_CASE'})
  await until('video and sanitized reply',async()=>(await items(chief.conversation)).some(i=>i.kind==='bot'&&i.text==='视频已生成。'))
  const video=(await items(chief.conversation)).find(i=>i.kind==='file'&&i.generated==='video')
  assert.ok(video)
  assert.equal(video.caption,undefined)
  assert.equal(videoRequests[0].prompt,privateVideoPrompt)
  assert.equal((await fetch(`${app.base}/${video.src}`,{headers:{cookie:app.cookie}})).status,200)
  assert.ok(events.some(e=>e.event==='work'&&Object.values(e.data.runs).some(r=>r.activity==='generating a video')))
  assert.equal(events.slice(videoEventsStart).some(e=>e.event==='chat.stream'&&e.data.conversation===chief.conversation),false,'video drafts must not stream')
  const beforeGroupVideos=videoRequests.length
  await call('chat.send',{conversation:group,text:`请委派生成视频 VIDEO_PRIVACY_CASE REUSE_BOT ${imageTask.target}`})
  const videoTask=await until('delegated video result',async()=>(await call('task.list')).find(t=>t.source.startsWith(group+':')&&t.text.includes('VIDEO_PRIVACY_CASE')&&t.status==='completed'))
  assert.equal(videoTask.target,imageTask.target)
  assert.equal(videoRequests.length,beforeGroupVideos+1)
  assert.equal(videoTask.result,'视频已生成。')
  assert.equal(videoTask.files[0].caption,undefined)
  assert.equal((await items(group)).filter(i=>i.kind==='file'&&i.generated==='video').length,1)
  await until('group video synthesis finished',async()=>!Object.keys((await call('team.state')).work.runs).some(k=>k.startsWith(group+':')))
  failVideo=true
  await idle(chief.conversation)
  await call('chat.send',{conversation:chief.conversation,text:'请生成一段视频 VIDEO_PRIVACY_CASE_FAIL'})
  await until('video reflected error sanitized',async()=>(await items(chief.conversation)).some(i=>i.kind==='bot'&&i.text.startsWith('视频生成未完成')))
  appendFileSync(join(app.home,'team','chats',chief.conversation.replaceAll(':','_')+'.jsonl'),JSON.stringify({...legacy,id:'legacy-video-fixture',src:video.src,name:'video-20261024-123456-ab12.mp4',mime:'video/mp4',caption:privateVideoPrompt})+'\n')
  const videoHistory=await items(chief.conversation)
  assert.equal(videoHistory.find(i=>i.id==='legacy-video-fixture').caption,undefined)
  assert.ok(!JSON.stringify(videoHistory).includes(privateVideoPrompt))
  assert.ok(!JSON.stringify(await items(group)).includes(privateVideoPrompt))
  assert.ok(!JSON.stringify(await call('team.state')).includes(privateVideoPrompt))
  assert.ok(!JSON.stringify(events).includes(privateVideoPrompt))
  console.log('ok   direct/group video generation hides drafts, prompts, provider reflections and legacy captions while delivering one downloadable attachment')
  const logs=readFileSync(app.log,'utf8')
  assert.match(logs,/Never publish generation prompts/)
  const launches=logs.trim().split('\n').map(line=>JSON.parse(line).args)
  const selection=launches.find(args=>args.includes('--json-schema')&&JSON.parse(args[args.indexOf('--json-schema')+1]).properties.delegationChoice)
  assert.equal(selection[selection.indexOf('--model')+1],'delegation-model','selection must use the coordinator model')
  const veteranRuns=launches.filter(args=>args.includes('--system-prompt')&&args[args.indexOf('--system-prompt')+1].includes('你是 ExistingReviewer，'))
  assert.ok(veteranRuns.length)
  assert.ok(veteranRuns.every(args=>args[args.indexOf('--model')+1]==='existing-model'),'reused workers must actually launch with their own model')
  console.log('ok   image progress/artifacts remain available; streamed drafts, captions, repeated final prompts, reflected errors and legacy captions do not expose generation prompts')
} finally {
  feedController.abort();await feedTask
  await app?.stop()
  if(imageServer.listening)await new Promise(resolve=>imageServer.close(resolve))
  console.log('ok   isolated test service, SSE reader and image fixture closed')
}
