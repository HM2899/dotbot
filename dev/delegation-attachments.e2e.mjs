#!/usr/bin/env node
// Actual task delivery, native inputs, file authorization and local byte copies.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { startServer, until } from './harness.mjs'

let app
try {
  app = await startServer({binary:process.env.DOTBOT_TEST_BINARY||undefined})
  const {call, base, cookie} = app
  await call('settings.learning',{enabled:false})
  const chief=(await call('team.state')).bots[0], chat=chief.conversation
  const reader=await call('bot.create',{name:'AttachmentReader',role:'Reader'})
  const second=await call('bot.create',{name:'SecondReader',role:'Reader'})
  const items=async c=>(await call('chat.page',{conversation:c})).items
  const idle=c=>until('source idle',async()=>!(await call('team.state')).work.runs[c])
  const upload=async(c,name,body)=>{
    const response=await fetch(`${base}/api/files?${new URLSearchParams({conversation:c,name})}`,{method:'POST',headers:{cookie,'content-type':'application/octet-stream'},body})
    assert.equal(response.status,200)
    return (await response.json()).file
  }
  const marker='delegated-attachment-bytes', text=await upload(chat,'资料.txt',marker)
  const image=await upload(chat,'reference.png',Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==','base64'))
  const pdf=await upload(chat,'资料.pdf','%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n%%EOF')
  const binary=Buffer.from('504b0506000000000000000000000000000000000000','hex')
  const zip=await upload(chat,'archive.zip',binary)
  const staged=await upload(chat,'never-sent.txt','never sent private marker')
  const all=[text.src,image.src,pdf.src,zip.src]
  const send=async(c,body,files=[])=>{
    await idle(c)
    const before=new Set((await call('task.list')).map(t=>t.id))
    await call('chat.send',{conversation:c,text:body,files})
    const task=await until('new delegated task',async()=>(await call('task.list')).find(t=>!before.has(t.id)))
    const completed=await until('delegated task completed',async()=>{
      for(const approval of (await call('team.state')).work.approvals) await call('approval.answer',{id:approval.id,allow:true})
      const current=await call('task.get',{id:task.id})
      if(current.status==='failed') throw new Error(current.result)
      return current.status==='completed'&&current
    })
    await idle(c)
    return completed
  }
  const tool=(name,args)=>`TOOL ${name} ${JSON.stringify(args)}`
  const read={bot:reader.name,text:`REPORT_ATTACHMENT ${marker}`}
  const inherited=await send(chat,tool('message_bot',read),all)
  assert.deepEqual(inherited.inputs,all)
  assert.equal(inherited.result,'Attachments: images=1 pdf=1; text=true')
  assert.ok(!inherited.inputs.includes(staged.src),'unsent uploads are never inherited')
  const selected=await send(chat,tool('message_bot',{...read,files:[pdf.src]}),all)
  assert.deepEqual(selected.inputs,[pdf.src])
  assert.equal(selected.result,'Attachments: images=0 pdf=1; text=false')
  const excluded=await send(chat,tool('message_bot',{...read,files:[]}),all)
  assert.deepEqual(excluded.inputs,[])
  assert.equal(excluded.result,'Attachments: images=0 pdf=0; text=false')
  const fresh=await send(chat,tool('create_bot',{name:'InitialTaskReader',role:'Reader',instructions:'Read the supplied attachments.',task:`REPORT_ATTACHMENT ${marker}`}),all)
  assert.deepEqual(fresh.inputs,all)
  assert.equal(fresh.result,'Attachments: images=1 pdf=1; text=true')
  console.log('ok   omitted files inherit native/text/binary inputs; subsets, empty lists and create_bot initial tasks preserve intent')

  const nested=await send(chat,tool('message_bot',{bot:reader.name,text:tool('message_bot',{bot:second.name,text:`REPORT_ATTACHMENT ${marker}`})}),all)
  const child=await until('nested input inherited',async()=>(await call('task.list')).find(t=>t.parent===nested.id&&t.status==='completed'))
  assert.deepEqual(child.inputs,all)
  assert.equal(child.result,'Attachments: images=1 pdf=1; text=true')
  const nestedExcluded=await send(chat,tool('message_bot',{bot:reader.name,text:tool('message_bot',{bot:second.name,text:`REPORT_ATTACHMENT ${marker}`}),files:[pdf.src]}),all)
  const selectedChild=await until('nested subset retained',async()=>(await call('task.list')).find(t=>t.parent===nestedExcluded.id&&t.status==='completed'))
  assert.deepEqual(selectedChild.inputs,[pdf.src],'children cannot recover attachments excluded by their parent')
  assert.equal(selectedChild.result,'Attachments: images=0 pdf=1; text=false')
  console.log('ok   nested delegation inherits only the actual parent inputs, preserving exclusions')

  const explicit=await send(chat,`请委派核查附件 REUSE_BOT ${reader.id}\nREPORT_ATTACHMENT ${marker}`,all)
  assert.deepEqual(explicit.inputs,all)
  assert.equal(explicit.result,'Attachments: images=1 pdf=1; text=true')
  const followup=await send(chat,`请委派处理刚才的附件 REUSE_BOT ${reader.id}\nREPORT_ATTACHMENT ${marker}`)
  assert.deepEqual(followup.inputs,all)
  assert.equal(followup.result,'Attachments: images=1 pdf=1; text=true')
  const room=await call('room.create',{name:'Attachment delegation group',members:[chief.id,reader.id],admin:chief.id})
  const group=`room:${room.id}`
  const groupFile=await upload(group,'group-only.txt',marker)
  for(const mode of ['auto','admin','all']) {
    await call('room.update',{id:room.id,mode})
    const task=await send(group,`请委派处理附件 REUSE_BOT ${reader.id}\nREPORT_ATTACHMENT ${marker}`,[groupFile.src])
    assert.deepEqual(task.inputs,[groupFile.src])
    assert.equal(task.targetConversation.split('~task~')[0],`${group}:${reader.id}`)
    assert.equal(task.result,'Attachments: images=0 pdf=0; text=true')
    await until('group seats idle',async()=>!Object.keys((await call('team.state')).work.runs).some(k=>k.startsWith(group+':')))
  }
  assert.equal((await items(`bot:${reader.id}`)).some(i=>i.src===groupFile.src),false,'group attachments stay out of the private chat')
  const unrelated=await call('bot.create',{name:'Unrelated',role:'Reader'})
  const unshared=await upload(`bot:${unrelated.id}`,'private.txt','unshared private bytes')
  const before=(await call('task.list')).length
  await call('chat.send',{conversation:chat,text:tool('message_bot',{...read,files:[unshared.src]})})
  await idle(chat)
  assert.equal((await call('task.list')).length,before,'explicit files still require conversation authorization')
  await call('chat.send',{conversation:chat,text:tool('message_bot',{...read,files:[42]})})
  await idle(chat)
  assert.equal((await call('task.list')).length,before,'invalid references must fail instead of silently dropping files')
  console.log('ok   explicit/follow-up delegation and every group mode retain inputs without private-chat leakage or invalid-file fallback')

  const unrelatedTask=await send(chat,tool('message_bot',{bot:reader.name,text:'REPORT_GROUP_CONTEXT unrelated_without_inputs'}))
  assert.deepEqual(unrelatedTask.inputs,[],'ordinary work must not inherit previous attachments')
  await idle(chat)
  const previousTasks=new Set((await call('task.list')).map(t=>t.id))
  await call('chat.send',{conversation:chat,text:`DELAY 1200\n${tool('message_bot',{bot:reader.name,text:'REPORT_GROUP_CONTEXT queue_isolation'})}`})
  await until('first source turn running',async()=>(await call('team.state')).work.runs[chat])
  const queuedFile=await upload(chat,'queued-only.txt','queued separate task data')
  await call('chat.send',{conversation:chat,text:'separate queued request',files:[queuedFile.src]})
  const isolated=await until('queued request excluded',async()=>(await call('task.list')).find(t=>!previousTasks.has(t.id)&&t.text.includes('queue_isolation')&&t.status==='completed'))
  assert.deepEqual(isolated.inputs,[],'later queued request attachments must not enter the active delegation')
  await idle(chat)
  console.log('ok   unrelated work and later queued requests do not contribute attachments')

  await call('settings.local-computer',{enabled:true})
  const copied=await send(chat,tool('message_bot',{bot:second.name,text:tool('local_receive_file',{file:zip.src})}),[zip.src])
  assert.deepEqual(copied.inputs,[zip.src])
  const destination=copied.result.match(/Local copy: ([^\n]+)/)?.[1]
  assert.ok(destination)
  assert.deepEqual(readFileSync(destination),binary,'the receiving Bot gets the original binary bytes')
  await call('settings.local-computer',{enabled:false})
  console.log('ok   inherited binary attachments can actually be copied into the receiving Bot local workspace')
} finally {
  await app?.stop()
  console.log('ok   isolated attachment delegation service cleaned up')
}
