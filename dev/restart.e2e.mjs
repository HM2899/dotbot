import assert from 'node:assert/strict'
import {mkdtempSync,rmSync,statSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {startServer,until} from './harness.mjs'

const directory=mkdtempSync(join(tmpdir(),'dotbot-restart-'))
let app
try {
 app=await startServer({directory,keepHome:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 const cookie=app.cookie,firstAddress=app.address
 const get=async(base,path,oldCookie=cookie)=>fetch(base+path,{headers:{cookie:oldCookie},redirect:'manual'})
 assert.match((await get(app.base,'/')).headers.get('content-type'),/html/)
 const login=await fetch(app.address,{redirect:'manual'})
 assert.equal(login.status,303)
 assert.equal(login.headers.get('location'),'/login')
 assert.equal(login.headers.get('set-cookie'),null)
 assert.equal(statSync(join(directory,'team/web-auth/admin.json')).mode&0o777,0o600)
 await assert.rejects(startServer({directory,keepHome:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}}),/exited/)
 let starts=0
 const onData=chunk=>{starts+=(chunk.toString().match(/open: /g)||[]).length}
 app.server.stdout.on('data',onData)
 app.server.kill('SIGUSR2')
 await until('service replaces itself at the same address',async()=>{
  if(!starts)return false
  try{return(await get(app.base,'/')).status===200}catch{return false}
 },30000)
 const feed=await get(app.base,'/api/bot/feed');assert.equal(feed.status,200);await feed.body.cancel()
 console.log('ok   SIGUSR2 restarts at the same port and preserves the existing authenticated cookie')
 await app.stop()
 app=await startServer({directory,keepHome:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 assert.equal((await get(app.base,'/')).status,200)
 assert.equal(new URL(app.address).search,new URL(firstAddress).search)
 console.log('ok   normal service restart preserves administrator sessions without startup token links')
 await app.stop()
 app=await startServer({directory,keepHome:true,extraArgs:['--rotate-login'],extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 assert.equal((await get(app.base,'/')).status,303)
 assert.equal((await get(app.base,'/api/bot/feed')).status,401)
 assert.equal((await get(app.base,'/',app.cookie)).status,200)
 console.log('ok   explicit credential rotation revokes old sign-ins')
}finally{await app?.stop();rmSync(directory,{recursive:true,force:true})}
