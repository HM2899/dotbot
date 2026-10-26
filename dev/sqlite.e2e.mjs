#!/usr/bin/env node
import assert from 'node:assert/strict'
import {mkdtempSync,rmSync,readFileSync,existsSync,writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {execFileSync} from 'node:child_process'
import {startServer,until} from './harness.mjs'
const directory=mkdtempSync(join(tmpdir(),'dotbot-sqlite-'))
let app
const start=storage=>startServer({directory,keepHome:true,storage,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
try{
 app=await start('json')
 const chief=(await app.call('team.state')).bots[0]
 const worker=await app.call('bot.create',{name:'SQLite 专家',role:'数据库',model:'kept-model'})
 await app.call('settings.local-computer',{enabled:true})
 await app.call('chat.send',{conversation:chief.conversation,text:'sqlite migration evidence'})
 await until('reply stored',async()=>(await app.call('chat.page',{conversation:chief.conversation})).items.some(i=>i.text==='Echo: sqlite migration evidence'))
 await app.stop()
 const snapshot=readFileSync(join(directory,'team/state.json'),'utf8')
 app=await start('sqlite')
 assert.ok(existsSync(join(directory,'team/dotbot.sqlite3')))
 assert.equal((await app.call('team.state')).bots.find(b=>b.id===worker.id).model,'kept-model')
 assert.equal((await app.call('local.status')).enabled,true)
 assert.ok((await app.call('chat.page',{conversation:chief.conversation})).items.some(i=>i.text==='Echo: sqlite migration evidence'))
 await app.call('bot.update',{id:worker.id,name:'数据库已迁移'})
 await app.call('chat.send',{conversation:chief.conversation,text:'sqlite restart evidence'})
 await until('SQLite reply',async()=>(await app.call('chat.page',{conversation:chief.conversation})).items.some(i=>i.text==='Echo: sqlite restart evidence'))
 await app.call('settings.local-computer',{enabled:false})
 await app.call('connection.save',{name:'tracker',kind:'http',url:'https://tracker.example.com/mcp'})
 assert.equal(readFileSync(join(directory,'team/state.json'),'utf8'),snapshot)
 assert.equal(existsSync(join(directory,'team/connections.json')),false)
 await app.stop()
 writeFileSync(join(directory,'team/state.json'),'stale invalid backup; database must win')
 app=await start('sqlite')
 const state=await app.call('team.state')
 assert.equal(state.bots.find(b=>b.id===worker.id).name,'数据库已迁移')
 assert.equal((await app.call('local.status')).enabled,false)
 assert.ok(state.connections.some(c=>c.name==='tracker'))
 assert.ok((await app.call('chat.page',{conversation:chief.conversation})).items.some(i=>i.text==='Echo: sqlite restart evidence'))
 const check=execFileSync('python',['-c',"import sqlite3,sys; d=sqlite3.connect(sys.argv[1]); assert d.execute('pragma quick_check').fetchone()[0]=='ok'; assert d.execute('select count(*) from documents').fetchone()[0]>=3; assert d.execute('select count(*) from chat_items').fetchone()[0]>=4; print('ok')",join(directory,'team/dotbot.sqlite3')],{encoding:'utf8'})
 assert.equal(check.trim(),'ok')
 await app.stop()
 await assert.rejects(start('json'),/exited/)
 console.log('ok   legacy metadata/chat migration, SQLite-only writes, unchanged backups, native model preservation, settings persistence, restart, integrity and stale-JSON rollback prevention')
}finally{await app?.stop();rmSync(directory,{recursive:true,force:true})}
