// Real commands on one temporary shared computer. No browsers or external services.
import assert from 'node:assert/strict'
import {execFileSync} from 'node:child_process'
import {existsSync,readFileSync,writeFileSync} from 'node:fs'
import {join} from 'node:path'
import {startServer,until} from './harness.mjs'
let app,container,timer,approvalWork=Promise.resolve(),closing=false
const docker=(...args)=>execFileSync('docker',args,{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim()
try {
 app=await startServer({desktop:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 const {call}=app,a=(await call('team.state')).bots[0],b=await call('bot.create',{name:'资源验证 B',role:'Tester'}),c=await call('bot.create',{name:'资源验证 C',role:'Tester'})
 container=(await call('desktop.status',{bot:a.id})).container
 for(const bot of[a,b,c])await call('desktop.open',{bot:bot.id})
 const home=join(app.home,'team/desktops/shared'),file=name=>join(home,name),chat=bot=>`bot:${bot.id}`
 const approve=()=>{approvalWork=approvalWork.then(async()=>{if(closing)return;for(const approval of(await call('team.state')).work.approvals)await call('approval.answer',{id:approval.id,allow:true})}).catch(()=>{})}
 timer=setInterval(approve,60)
 const run=async(bot,command,resources=[])=>call('chat.send',{conversation:chat(bot),text:'TOOL desktop_run '+JSON.stringify({command,resources,timeout_s:60})})
 const idle=async bot=>!(await call('team.state')).work.runs[chat(bot)]
 await run(a,'printf started >/config/resource-a.started; while [ ! -e /config/resource-a.release ]; do sleep .1; done; printf A >/config/coordinated-document.txt',['/config/coordinated-document.txt'])
 await until('first writer holds shared document',()=>existsSync(file('resource-a.started')),30000)
 await run(b,'printf B >/config/coordinated-document.txt; printf done >/config/resource-b.done',['/config/coordinated-document.txt'])
 await run(c,'printf independent >/config/independent-result.txt',['/config/independent-result.txt'])
 await until('another project runs while document is reserved',()=>existsSync(file('independent-result.txt')),30000)
 assert.equal(existsSync(file('resource-b.done')),false)
 writeFileSync(file('resource-a.release'),'release')
 await until('waiting writer proceeds after release',()=>existsSync(file('resource-b.done')),30000)
 assert.equal(readFileSync(file('coordinated-document.txt'),'utf8'),'B')
 console.log('ok   overlapping shared-file commands wait; independent project commands continue')
 await until('initial writers finish',async()=>await idle(a)&&await idle(b)&&await idle(c),30000)
 await run(a,'printf holding >/config/resource-hold.started; sleep 30; printf forbidden >/config/resource-after-stop.txt',['/config/coordinated-document.txt'])
 await until('long-running command started',()=>existsSync(file('resource-hold.started')),30000)
 await run(b,'printf forbidden >/config/waiter-after-stop.txt',['/config/coordinated-document.txt'])
 await until('second writer waits for the resource',async()=>(await call('desktop.status',{bot:b.id})).resourceWaiters>0,30000)
 await call('chat.stop',{conversation:chat(b)})
 await call('chat.stop',{conversation:chat(a)})
 await until('stopped turns settle',async()=>await idle(a)&&await idle(b),30000)
 await run(c,'printf continued >/config/resource-c.done',['/config/coordinated-document.txt'])
 await until('resources released after cancellation',()=>existsSync(file('resource-c.done')),30000)
 assert.equal(existsSync(file('waiter-after-stop.txt')),false)
 assert.equal(existsSync(file('resource-after-stop.txt')),false)
 assert.ok(!docker('exec',container,'ps','-eo','args').split('\n').some(line=>line.includes('bash -lc')&&line.includes('resource-hold.started')))
 console.log('ok   stopping cancels resource waiters and their own running command group before releasing reservations')
 await until('final command settles',()=>idle(c),30000)
 writeFileSync(file('upgrade-fixture.html'),'<title>升级保留窗口验证</title><h1>Keep this task-owned window open</h1>')
 docker('exec','-d','-u','abc','-w','/config',container,'python3','-m','http.server','8847','--bind','127.0.0.1')
 await call('desktop.control',{bot:a.id,user:true})
 await call('desktop.launch',{bot:a.id,app:'browser',url:'http://127.0.0.1:8847/upgrade-fixture.html'})
 const status=await call('desktop.status',{bot:a.id}),profile=`/config/.teambot/s${status.display.slice(1)}/browser`
 const browserPid=()=>docker('exec',container,'python3','-c',"import sys,json\nfrom pathlib import Path\nprint(json.dumps([p.name for p in Path('/proc').iterdir() if p.name.isdigit() and (p/'cmdline').exists() and ('--user-data-dir='+sys.argv[1]).encode() in (p/'cmdline').read_bytes().split(b'\\0') and not any(a.startswith(b'--type=') for a in (p/'cmdline').read_bytes().split(b'\\0'))]))",profile)
 const beforePid=await until('task-owned visible browser starts',()=>{try{const p=browserPid();return p!=='[]'&&p}catch{return false}},30000)
 const beforeContainer=docker('inspect','-f','{{.State.StartedAt}}',container)
 let restarted=false
 app.server.stdout.on('data',chunk=>{if(chunk.toString().includes('open: '))restarted=true})
 app.server.kill('SIGUSR2')
 await until('upgraded service accepts the existing app cookie',async()=>{if(!restarted)return false;try{return(await call('team.state')).desktop}catch{return false}},30000)
 await call('desktop.open',{bot:a.id})
 assert.equal(docker('inspect','-f','{{.State.StartedAt}}',container),beforeContainer)
 assert.equal(browserPid(),beforePid)
 assert.equal((await call('desktop.status',{bot:a.id})).user,true)
 await call('desktop.control',{bot:a.id,user:false})
 console.log('ok   upgrade restart preserves the actual browser PID, shared container, sign-in cookie and active user takeover')
 await run(b,'printf started >/config/restart-command.started; sleep 30; printf forbidden >/config/restart-command-after.txt',['/config/restart-command-after.txt'])
 await until('active command exists before upgrade',()=>existsSync(file('restart-command.started')),30000)
 restarted=false;app.server.kill('SIGUSR2')
 await until('upgrade settles active command before replacing runtime',async()=>{if(!restarted)return false;try{return(await call('team.state')).desktop}catch{return false}},30000)
 assert.equal(existsSync(file('restart-command-after.txt')),false)
 assert.ok(!docker('exec',container,'ps','-eo','args').split('\n').some(line=>line.includes('bash -lc')&&line.includes('restart-command.started')))
 assert.equal(browserPid(),beforePid)
 console.log('ok   upgrade interrupts active command safely, retains interruption evidence and preserves the unrelated browser')
}finally{
 closing=true;clearInterval(timer);await approvalWork
 await app?.stop()
 if(container){try{docker('rm','-f',container)}catch(e){if(!e.stderr?.toString().includes('No such'))throw e}}
}
