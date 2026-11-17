import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync,mkdirSync,writeFileSync,readFileSync,readlinkSync,symlinkSync,existsSync,rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
const installer=fileURLToPath(new URL('../../scripts/install-cli.sh',import.meta.url));
function fixture(run){const dir=mkdtempSync(join(tmpdir(),'hession-cli-link-'));const app=join(dir,"Hession ' $(touch UNEXPECTED).app");mkdirSync(app);const binary=join(app,'hession');writeFileSync(binary,'#!/bin/sh\nprintf "%s\\n" "$@"\n',{mode:0o755});const destination=join(dir,'bin','hession');const install=(source=binary,target=destination)=>spawnSync('sh',[installer,source,target],{encoding:'utf8',cwd:dir});try{run({dir,binary,destination,install});}finally{rmSync(dir,{recursive:true,force:true});}}
test('optional bundle link preserves literal paths and forwards arguments',()=>fixture(({dir,binary,destination,install})=>{const result=install();assert.equal(result.status,0,result.stderr);assert.equal(readlinkSync(destination),binary);const run=spawnSync(destination,['status','arg with spaces','$(touch UNEXPECTED)'],{encoding:'utf8',cwd:dir});assert.equal(run.status,0,run.stderr);assert.equal(run.stdout,'status\narg with spaces\n$(touch UNEXPECTED)\n');assert(!existsSync(join(dir,'UNEXPECTED')));assert.equal(install().status,0);}));
test('optional link never replaces an unrelated command or dangling symlink',()=>fixture(({destination,install})=>{mkdirSync(join(destination,'..'),{recursive:true});writeFileSync(destination,'unrelated command');assert.notEqual(install().status,0);assert.equal(readFileSync(destination,'utf8'),'unrelated command');rmSync(destination);symlinkSync('/synthetic/missing',destination);assert.notEqual(install().status,0);assert.equal(readlinkSync(destination),'/synthetic/missing');}));
test('optional link rejects invalid paths before creating a destination',()=>fixture(({destination,install})=>{assert.equal(install('relative/hession').status,2);assert(!existsSync(join(destination,'..')));assert.equal(install('/synthetic/not-installed').status,2);assert(!existsSync(join(destination,'..')));}));
