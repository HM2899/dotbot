#!/usr/bin/env node
// A stopped, task-owned legacy computer proves migration without touching user desktops.
import assert from 'node:assert/strict'
import {createHash} from 'node:crypto'
import {execFileSync} from 'node:child_process'
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {startServer} from './harness.mjs'
const home=mkdtempSync(join(tmpdir(),'dotbot-shared-migration-'))
const desktops=join(home,'team/desktops'),oldHome=join(desktops,'bot-legacy-donor'),otherHome=join(desktops,'bot-legacy-other')
const key=createHash('sha256').update(`home:${desktops}`).digest('hex').slice(0,32)
const shared=`teambot-computer-${key}`,legacy=`teambot-desktop-test-${key}`,image=`${shared}:migration`
const docker=(...args)=>execFileSync('docker',args,{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim()
let app
try{
 mkdirSync(oldHome,{recursive:true});mkdirSync(otherHome,{recursive:true})
 writeFileSync(join(oldHome,'preserved.txt'),'legacy-home-ok')
 writeFileSync(join(otherHome,'other.txt'),'other-home-ok')
 const tool=join(home,'legacy-tool');writeFileSync(tool,'#!/bin/sh\nprintf legacy-software-ok\n',{mode:0o755})
 docker('create','--name',legacy,'--label','teambot.desktop=1','--label',`teambot.home=${key}`,'-v',`${oldHome}:/config`,'lscr.io/linuxserver/webtop:ubuntu-xfce')
 docker('cp',tool,`${legacy}:/usr/local/bin/dotbot-legacy-check`)
 app=await startServer({directory:home,keepHome:true,desktop:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 const {call}=app,bot=(await call('team.state')).bots[0]
 await call('desktop.open',{bot:bot.id})
 assert.equal((await call('desktop.status',{bot:bot.id})).container,shared)
 assert.equal(docker('exec',shared,'dotbot-legacy-check'),'legacy-software-ok')
 assert.equal(docker('exec','-u','abc',shared,'cat','/config/preserved.txt'),'legacy-home-ok')
 assert.ok(!docker('exec',shared,'cat','/proc/mounts').includes('/config/legacy'),'old Bot homes must not be mounted (file dialogs list mounts as drives)')
 assert.equal(readFileSync(join(otherHome,'other.txt'),'utf8'),'other-home-ok','old Bot files stay on the host')
 assert.equal(docker('inspect','-f','{{.State.Status}}',legacy),'created','legacy container must remain untouched')
 assert.equal(readFileSync(join(oldHome,'preserved.txt'),'utf8'),'legacy-home-ok')
 const manifest=JSON.parse(readFileSync(join(desktops,'migration.json'),'utf8'))
 assert.equal(manifest.image,image);assert.ok(manifest.source)
 const display=(await call('desktop.status',{bot:bot.id})).display
 await app.stop();app=null
 app=await startServer({directory:home,keepHome:true,desktop:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 await app.call('desktop.open',{bot:bot.id})
 assert.equal((await app.call('desktop.status',{bot:bot.id})).display,display)
 assert.equal(docker('exec',shared,'dotbot-legacy-check'),'legacy-software-ok')
 console.log('ok   legacy software/home migration, no old-home mounts, original-container preservation and service restart')
 await app.stop();app=null
 docker('rm',shared);docker('image','rm',image)
 app=await startServer({directory:home,keepHome:true,desktop:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 await app.call('desktop.open',{bot:bot.id})
 assert.equal(docker('inspect','-f','{{.Config.Image}}',shared),'lscr.io/linuxserver/webtop:ubuntu-xfce')
 assert.equal(docker('exec','-u','abc',shared,'cat','/config/preserved.txt'),'legacy-home-ok')
 assert.ok(!docker('exec',shared,'cat','/proc/mounts').includes('/config/legacy'))
 assert.equal(docker('inspect','-f','{{.State.Status}}',legacy),'created')
 console.log('ok   absent old-host migration image falls back to native configured image without deleting shared or legacy files')
 await app.stop();app=null
 // A stopped computer from an older version that still mounts an old Bot home.
 docker('rm',shared)
 docker('create','--name',shared,'--label','teambot.desktop=1','--label',`teambot.home=${key}`,'--label','teambot.shared=1','-v',`${join(desktops,'shared')}:/config`,'-v',`${otherHome}:/config/legacy/bot-legacy-other:ro`,'lscr.io/linuxserver/webtop:ubuntu-xfce')
 docker('cp',tool,`${shared}:/usr/local/bin/dotbot-installed-later`)
 app=await startServer({directory:home,keepHome:true,desktop:true,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 await app.call('desktop.open',{bot:bot.id})
 assert.ok(!docker('exec',shared,'cat','/proc/mounts').includes('/config/legacy'),'an old-layout computer is recreated without old-home mounts')
 assert.equal(docker('exec',shared,'dotbot-installed-later'),'legacy-software-ok','software installed in the old-layout computer survives')
 assert.equal(docker('exec','-u','abc',shared,'cat','/config/preserved.txt'),'legacy-home-ok')
 assert.equal(readFileSync(join(otherHome,'other.txt'),'utf8'),'other-home-ok')
 console.log('ok   stopped old-layout computer keeps its software and drops old-home mounts')
}finally{
 await app?.stop()
 const failures=[]
 for(const name of [shared,legacy]){
  try{
   const mounts=JSON.parse(docker('inspect','-f','{{json .Mounts}}',name))
   assert.ok(mounts.some(m=>m.Source.startsWith(home+'/')))
   docker('rm','-f',name)
  }catch(e){if(!e.stderr?.toString().includes('No such'))failures.push(e)}
 }
 try{docker('image','rm',image)}catch(e){if(!e.stderr?.toString().includes('No such'))failures.push(e)}
 rmSync(home,{recursive:true,force:true})
 if(failures.length)throw new AggregateError(failures,'Shared desktop migration cleanup failed')
}
