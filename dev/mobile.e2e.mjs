#!/usr/bin/env node
// Flutter's real HTTP client talks to an isolated authenticated SQLite server.
import {spawn} from 'node:child_process'
import {startServer,root} from './harness.mjs'
let app
try{
 const password='mobile-contract-fixture-123'
 const bootstrap=process.env.DOTBOT_TEST_SETUP==='1'
 app=await startServer({storage:'sqlite',bootstrap,adminUsername:bootstrap?'admin':'mobile.owner',adminPassword:bootstrap?undefined:password,extraEnv:{DOTBOT_SKILL_DEFAULTS:'0'}})
 const child=spawn(process.env.DOTBOT_FLUTTER||'flutter',['test','--no-pub','test/server_contract_test.dart'],{cwd:root+'/mobile',env:{...process.env,DOTBOT_E2E_URL:app.base,DOTBOT_E2E_PASSWORD:bootstrap?app.initialPassword:password,DOTBOT_E2E_USERNAME:bootstrap?'admin':'mobile.owner'},stdio:'inherit'})
 const code=await new Promise((resolve,reject)=>{child.once('exit',resolve);child.once('error',reject)})
 if(code!==0)throw new Error('Flutter server contract failed')
 console.log('ok   native Flutter client verified against authenticated dotbot SQLite server')
}finally{await app?.stop()}
