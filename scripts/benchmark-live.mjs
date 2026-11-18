// Immutable results and exact byte bounds; no cross-call size cache.
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const source = process.argv[2] ? pathToFileURL(resolve(process.argv[2])) : new URL('../src/live-state.js',import.meta.url);
const { mergeLiveEntries, MAX_LIVE_BYTES } = await import(source);
const event = id => ({id,ordinal:id*11,kind:'message',role:'assistant',parts:[{kind:'text',text:'Event '+id+' '+'x'.repeat(4096)}],tools:[]});
const previous=Array.from({length:300},(_,i)=>event(i+1));
const oversized={...event(1),parts:[{kind:'text',text:'界'.repeat(MAX_LIVE_BYTES/2)}]};
const reports=[];
for(const [scenario, retained, incoming] of [
  ['empty',previous,[]], ['append-one',previous,[event(301)]],
  ['unchanged-100',previous,structuredClone(previous.slice(-100))],
  ['replay-same-id-100',previous,Array.from({length:100},()=>event(300))],
  ['duplicate-new-id-100',previous,Array.from({length:100},()=>event(301))],
  ['oversized-replay',[oversized],[structuredClone(oversized)]],
]){
  const samples=[];let result;
  for(let run=0;run<10;run++){
    const start=performance.now(); result=mergeLiveEntries(retained,incoming);
    if(run>=3)samples.push(performance.now()-start);
  }
  assert(result.entries.length<=300);
  assert(new TextEncoder().encode(JSON.stringify(result.entries)).byteLength<=MAX_LIVE_BYTES);
  if(scenario==='oversized-replay')assert.equal(result.entries.length,0);
  let serializations=0;const stringify=JSON.stringify;
  try{
    JSON.stringify=(...args)=>{serializations++;return stringify(...args);};
    result=mergeLiveEntries(retained,incoming);
  }finally{JSON.stringify=stringify;}
  samples.sort((a,b)=>a-b);
  reports.push({scenario,medianMs:samples[3],samplesMs:samples,serializations,added:result.added,updated:result.updated,rows:result.entries.length,reusedWindow:result.entries===retained,reusedRows:result.entries.filter(entry=>retained.includes(entry)).length});
}
console.log(JSON.stringify({source:source.pathname,cases:reports},null,2));
