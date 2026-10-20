import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {
  Activity, ArrowRight, ArrowUpDown, Bot, Check, ChevronDown, Copy,
  CircleHelp, Code2, Command, FileCode2, Filter, Folder, GitBranch,
  Layers3, LoaderCircle, Menu, MessageSquareText, MoreHorizontal, PanelLeftClose, RefreshCw,
  Search, Sparkles, Sun, Moon, Terminal, X, ChevronLeft, ChevronRight,
  TrendingUp, User, Star, Download, Square, Database, Server, Laptop,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { exportFilename } from './export.js';
import { BRAND, READY_EVENT, storageKey } from './brand.js';
import { listen } from '@tauri-apps/api/event';
import ExportSelect from './components/ExportSelect.jsx';
import SyncMenu from './components/SyncMenu.jsx';
import DataManagement from './components/DataManagement.jsx';
import RemoteHosts from './components/RemoteHosts.jsx';
import BrandMark from './components/BrandMark.jsx';
import AnalyticsView from './Analytics.jsx';
import { readNavigation, writeNavigation } from './navigation.js';
import DateFilter from './components/DateFilter.jsx';
import ModelFilter from './components/ModelFilter.jsx';
import BranchFilter from './components/BranchFilter.jsx';
import ToolFilter from './components/ToolFilter.jsx';
import './style.css';
import { hasToolActivity, isSessionEvent, transcriptParts } from './transcript.js';

import { parseDate, filterSince, calendarDayDifference } from './dates.js';

const API='/api';
const PAGE_SIZE=20;
const DETAIL_PAGE_SIZE=100; // H6: transcript pagination default
const req=async(url,opts={})=>{const r=await fetch(url,opts);const d=await r.json();if(!r.ok)throw new Error(d.error||r.statusText);return d;};
// Tauri-first API layer: try `invoke` first (desktop app); fall back to the
// HTTP dev API when the Tauri runtime is absent (vite dev outside Tauri).
// Command names follow the frontend<->backend contract:
// GET /api/health->get_health, /api/sessions->search_sessions,
// /api/sessions/:id->get_session, /api/agents->get_agents,
// /api/projects->get_projects, /api/facets->get_facets, /api/stats->get_stats,
// /api/sync/status->get_sync_status, /api/sync/checkpoints->get_sync_checkpoints,
// POST /api/sync->start_sync. Same query params are passed as invoke args.
const hasTauriRuntime=()=>typeof window!=='undefined'&&!!window.__TAURI_INTERNALS__?.invoke;
const tauriError=(e)=>e instanceof Error?e:new Error(typeof e==='string'?e:(e?.message||JSON.stringify(e??'Unknown error')));
const apiCall=async(cmd,args,url,opts={})=>{
  try{return await invoke(cmd,args);}
  catch(e){
    if(url==null||hasTauriRuntime())throw tauriError(e);
    return req(url,opts);
  }
};
// Maps the metadata endpoint suffix used by loadMeta to its Tauri command.
const META_COMMANDS={
  agents:'get_agents',
  projects:'get_projects',
  stats:'get_stats',
  facets:'get_facets',
  'sync/checkpoints':'get_sync_checkpoints',
};
const agentColors={codex:'lime',claude:'peach',opencode:'blue',pi:'violet',commandcode:'mint',muse:'pink',hermes:'amber',antigravity:'blue',cline:'mint'};
const agentNames={codex:'Codex',claude:'Claude Code',opencode:'OpenCode',pi:'Pi',commandcode:'Command Code',muse:'Muse',hermes:'Hermes Agent',antigravity:'Antigravity',cline:'Cline'};
const agentMark={codex:'C',claude:'✳',opencode:'⌘',pi:'π',commandcode:'⌁',muse:'M',hermes:'H',antigravity:'A',cline:'CL'};
const shortPath=(p)=>String(p??'').replace(/\\/g,'/').split('/').filter(Boolean).at(-1)||'Local workspace';
const relativeAgo=(value,now=Date.now())=>{if(!value)return 'never';const d=parseDate(value);if(!d||isNaN(d.getTime()))return 'never';const minutes=Math.max(0,Math.floor((now-d.getTime())/60000));if(minutes<1)return 'just now';if(minutes<60)return `${minutes} minute${minutes===1?'':'s'} ago`;const hours=Math.floor(minutes/60);if(hours<24)return `${hours} hour${hours===1?'':'s'} ago`;const days=Math.floor(hours/24);return `${days} day${days===1?'':'s'} ago`;};
const formatSyncTime=(value,now)=>{if(!value)return 'Never';const d=parseDate(value);if(!d||isNaN(d.getTime()))return 'Never';return `${d.toLocaleTimeString([],{hour:'numeric',minute:'2-digit'})} (${relativeAgo(d,now)})`;};
const relativeDate=(s)=>{if(!s)return 'Date unavailable';const d=parseDate(s),now=new Date();if(!d)return 'Date unavailable';const days=calendarDayDifference(d,now);if(days===0)return `Today · ${d.toLocaleTimeString([],{hour:'numeric',minute:'2-digit'})}`;if(days===1)return 'Yesterday';if(days>=0&&days<7)return d.toLocaleDateString([],{weekday:'long'});return d.toLocaleDateString([],{month:'short',day:'numeric',year:d.getFullYear()!==now.getFullYear()?'numeric':undefined});};
const familyLabel=(f)=>agentNames[f]||f||'Agent';

function App(){
  useEffect(()=>{window.dispatchEvent(new Event(READY_EVENT));},[]);
  const [sessions,setSessions]=useState([]),[total,setTotal]=useState(0),[agents,setAgents]=useState([]),[projects,setProjects]=useState([]);
  const [syncCheckpoints,setSyncCheckpoints]=useState([]),[syncMenuOpen,setSyncMenuOpen]=useState(false);
  const [stats,setStats]=useState({sessions:0,entries:0,agents:0,files:0}),[detail,setDetail]=useState(null);
  const [query,setQuery]=useState(''),[agentFilter,setAgentFilter]=useState(''),[projectFilter,setProjectFilter]=useState('');
  // '' = every machine, '@local' = this computer, otherwise an SSH host.
  const [hostFilter,setHostFilter]=useState(''),[remoteHostsOpen,setRemoteHostsOpen]=useState(false);
  const [starredOnly,setStarredOnly]=useState(false),[tagFilters,setTagFilters]=useState([]);
  const [tagDraft,setTagDraft]=useState(''),[annotationBusy,setAnnotationBusy]=useState(false);
  const [exportOpen,setExportOpen]=useState(false),[exportScope,setExportScope]=useState('filtered'),[exportFormat,setExportFormat]=useState('markdown'),[exportBusy,setExportBusy]=useState(false);
  const [exportProgress,setExportProgress]=useState({phase:'',done:0,total:0}),[exportStopping,setExportStopping]=useState(false);
  const exportDialogRef=useRef(null);
  useEffect(()=>{
    if(!exportOpen)return;
    const previous=document.activeElement;
    exportDialogRef.current?.querySelector('.export-select button')?.focus();
    return()=>{if(previous?.isConnected)previous.focus();};
  },[exportOpen]);
  const exportJobRef=useRef(null),starPendingRef=useRef(new Set());
  const [starPending,setStarPending]=useState(()=>new Set());
  const [includeStats,setIncludeStats]=useState(()=>localStorage.getItem(storageKey('export-stats'))==='true');
  const [sortOrder,setSortOrder]=useState('recent'),[page,setPage]=useState(0);
  const [sortOpen,setSortOpen]=useState(false),[facets,setFacets]=useState({models:[],branches:[],tools:[]});
  const [allWorkspaces,setAllWorkspaces]=useState(false),[workspaceQuery,setWorkspaceQuery]=useState(''),[workspaceMenuOpen,setWorkspaceMenuOpen]=useState(false);
  const [timeFilter,setTimeFilter]=useState(''),[modelFilters,setModelFilters]=useState([]),[branchFilter,setBranchFilter]=useState(''),[toolFilters,setToolFilters]=useState([]);
  const [sync,setSync]=useState({running:false}),[syncError,setSyncError]=useState('');
  const [syncStarting,setSyncStarting]=useState(false),[syncStopping,setSyncStopping]=useState(false);
  const [autoSync,setAutoSync]=useState(()=>{try{return localStorage.getItem(storageKey('auto-sync'))!=='false';}catch{return true;}});
  const [libraryReady,setLibraryReady]=useState(false),[syncStatusReady,setSyncStatusReady]=useState(false);
  const startupSyncHandled=useRef(false),syncLaunchPending=useRef(false);
  const toggleAutoSync=()=>{
    const next=!autoSync;
    try{localStorage.setItem(storageKey('auto-sync'),String(next));}catch{setNotice('Could not save the startup sync preference.');}
    setAutoSync(next);
  };
  const [mobileNav,setMobileNav]=useState(false);
  const [dataManagementOpen,setDataManagementOpen]=useState(false);
  const [resumeOpen,setResumeOpen]=useState(false),[resumeBusy,setResumeBusy]=useState(false),[resumeInfo,setResumeInfo]=useState(null),[resumeError,setResumeError]=useState(''),[resumeCopied,setResumeCopied]=useState(false),[resumeTerminals,setResumeTerminals]=useState([]),[resumeTerminal,setResumeTerminal]=useState('');
  const resumeDialogRef=useRef(null);
  const [loading,setLoading]=useState(true),[detailLoading,setDetailLoading]=useState(false),[metaLoaded,setMetaLoaded]=useState(false),[notice,setNotice]=useState('');
  const [detailError,setDetailError]=useState('');
  const [entryFilter,setEntryFilter]=useState('all'); // 'all' | 'user' | 'assistant' | 'tool' | 'event'
  const [detailPage,setDetailPage]=useState(1); // H6: 1-based transcript page
  const [detailJump,setDetailJump]=useState('');
  const [collapsedEntries,setCollapsedEntries]=useState(()=>new Set());
  const [copiedEntry,setCopiedEntry]=useState(null);
  const [theme,setTheme]=useState(()=>localStorage.getItem(storageKey('theme'))==='light'?'light':'dark');
  const [currentTime,setCurrentTime]=useState(Date.now());
  const [reportRevision,setReportRevision]=useState(0);
  const [navigation,setNavigation]=useState(readNavigation);
  const navigationRef=useRef(navigation);
  const sessionRequest=useRef(0),detailRequest=useRef(0),sortAnchorRef=useRef(null);
  // Mirrors whether a sync is in flight. Background refreshes read it so
  // lock-contention failures during sync stay silent instead of wiping UI.
  const syncActiveRef=useRef(false);
  useEffect(()=>{syncActiveRef.current=sync.running||syncStarting;});
  const {view,selected}=navigation;
  const navigate=useCallback((patch,{replace=false}={})=>{
    const next={...navigationRef.current,...patch};
    writeNavigation(next,{replace});
    navigationRef.current=next;
    setNavigation(next);
  },[]);
  const setView=nextView=>navigate({view:nextView});
  const selectSession=useCallback(sessionId=>{
    if(navigationRef.current.view==='sessions'&&String(navigationRef.current.selected)===String(sessionId))return;
    // Invalidate the previous detail request before rendering the new selection.
    detailRequest.current++;
    setDetailPage(1);setDetailJump('');setDetail(null);setDetailError('');setDetailLoading(true);
    navigate({view:'sessions',selected:sessionId,selectionExplicit:true});
    setMobileNav(false);
  },[navigate]);
  const updateAnalyticsNavigation=useCallback(patch=>{
    navigate({analytics:{...navigationRef.current.analytics,...patch}},{replace:true});
  },[navigate]);
  useEffect(()=>{
    writeNavigation(navigationRef.current,{replace:true});
    const handlePopState=()=>{
      const next=readNavigation();
      navigationRef.current=next;
      detailRequest.current++;
      setDetailPage(1);setDetailJump('');setDetail(null);setDetailError('');
      setNavigation(next);
      setMobileNav(false);setSortOpen(false);setSyncMenuOpen(false);
    };
    window.addEventListener('popstate',handlePopState);
    return()=>window.removeEventListener('popstate',handlePopState);
  },[]);

  const toggleCollapse=useCallback((id)=>{
    setCollapsedEntries(prev=>{
      const next=new Set(prev);
      if(next.has(id))next.delete(id);
      else next.add(id);
      return next;
    });
  },[]);

  const toggleTheme=()=>setTheme(current=>{
    const next=current==='dark'?'light':'dark';
    localStorage.setItem(storageKey('theme'),next);
    return next;
  });

  const copyEntry=async(entry)=>{
    try{await navigator.clipboard.writeText(entryAsMarkdown(entry));setCopiedEntry(entry.id);setTimeout(()=>setCopiedEntry(current=>current===entry.id?null:current),1400);}
    catch{setNotice('Could not access the clipboard. Check browser permissions and try again.');}
  };

  const openResume=async()=>{
    if(!detail||resumeBusy)return;
    setResumeOpen(true);setResumeBusy(true);setResumeError('');setResumeInfo(null);setResumeCopied(false);
    try{
      const [terms,info]=await Promise.all([
        apiCall('list_terminals',{},null).catch(()=>null),
        apiCall('get_resume_info',{id:String(detail.session.id)},null),
      ]);
      const list=terms?.terminals||[];
      setResumeTerminals(list);
      setResumeTerminal(info?.terminalId||terms?.saved||terms?.defaultId||'');
      setResumeInfo(info);
    }catch(e){setResumeError(e.message||'Could not prepare resume command.');}
    finally{setResumeBusy(false);}
  };
  const changeResumeTerminal=async(value)=>{
    if(!value||value===resumeTerminal)return;
    setResumeTerminal(value);
    try{await apiCall('set_terminal_preference',{terminal:value},null);}
    catch(e){setNotice(e.message);}
    if(!detail)return;
    setResumeBusy(true);setResumeError('');
    try{
      const info=await apiCall('get_resume_info',{id:String(detail.session.id),terminal:value},null);
      setResumeInfo(info);
    }catch(e){setResumeError(e.message||'Could not prepare resume command.');}
    finally{setResumeBusy(false);}
  };
  const launchResume=async()=>{
    if(!detail||resumeBusy)return;
    setResumeBusy(true);setResumeError('');
    try{
      const result=await apiCall('resume_session',{id:String(detail.session.id),terminal:resumeTerminal||null},null);
      if(result?.ok){
        setResumeOpen(false);setResumeInfo(null);
        setNotice(`Resumed in ${result.terminal||'terminal'}.`);
      }else{
        if(result&&typeof result==='object')setResumeInfo(result);
        setResumeError(result?.message||'Could not launch the session.');
      }
    }catch(e){setResumeError(e.message||'Could not launch the session.');}
    finally{setResumeBusy(false);}
  };
  const openInEditor=async()=>{
    if(!detail)return;
    try{await apiCall('open_in_editor',{id:String(detail.session.id)},null);setNotice(detail.session.host?`Opening in VS Code on ${detail.session.host}…`:'Opening in VS Code…');}
    catch(e){setNotice(e.message||'Could not open VS Code.');}
  };
  const copyResumeCommand=async()=>{
    const text=resumeInfo?.displayCommand||resumeInfo?.display_command||'';
    if(!text)return;
    try{await navigator.clipboard.writeText(text);setResumeCopied(true);setTimeout(()=>setResumeCopied(false),1400);}
    catch{setNotice('Could not access the clipboard. Check browser permissions and try again.');}
  };
  useEffect(()=>{setResumeOpen(false);setResumeInfo(null);setResumeError('');setResumeCopied(false);setResumeTerminals([]);setResumeTerminal('');},[selected]);

  const [metadataState,setMetadataState]=useState({agents:'loading',projects:'loading',stats:'loading',facets:'loading','sync/checkpoints':'loading'});
  const metadataRequest=useRef(0);
  const metadataStatus=(key,label)=>metadataState[key]==='loading'?<span className="metadata-status" role="status"><LoaderCircle className="spin" size={12}/>{label}</span>:metadataState[key]==='error'?<span className="metadata-status" role="status">Could not load {label.toLowerCase().replace('loading ','')}<button type="button" onClick={loadMeta}>Retry</button></span>:null;
  // Database lock contention while sync holds bulk write transactions.
  // Ignored when a sync is running; successes still update the UI normally.
  const isSyncLockError=e=>/database is (locked|busy)|database table is locked/i.test(String(e?.message??e??''));
  const loadMeta=useCallback(async({silent=false}={})=>{
    const id=++metadataRequest.current;
    if(!silent)setMetadataState({agents:'loading',projects:'loading',stats:'loading',facets:'loading','sync/checkpoints':'loading'});
    const update=async(endpoint,setter)=>{
      try{const data=await apiCall(META_COMMANDS[endpoint],{},`${API}/${endpoint}`);if(id===metadataRequest.current){setter(data);setMetadataState(current=>({...current,[endpoint]:'ready'}));}}
      catch(e){if(id===metadataRequest.current){if(syncActiveRef.current&&isSyncLockError(e))return;setNotice(e.message);setMetadataState(current=>({...current,[endpoint]:'error'}));}}
    };
    await Promise.all([
      update('agents',setAgents),update('projects',setProjects),update('stats',setStats),
      update('facets',setFacets),update('sync/checkpoints',data=>{setSyncCheckpoints(data);setMetaLoaded(true);}),
    ]);
  },[]);
  const loadSessions=useCallback(async({silent=false}={})=>{
    const requestId=++sessionRequest.current;
    if(!silent)setLoading(true);
    try{
      const sessionParams={q:query,host:hostFilter,agent:agentFilter,project:projectFilter,model:modelFilters.join(','),branch:branchFilter,tool:toolFilters.join(','),since:filterSince(timeFilter),sort:sortOrder,starred:String(starredOnly),tags:tagFilters,limit:String(PAGE_SIZE),offset:String(page*PAGE_SIZE)};
      const result=await apiCall('search_sessions',sessionParams,`${API}/sessions?${new URLSearchParams(sessionParams)}`);
      if(requestId!==sessionRequest.current)return;
      setSessions(result.sessions||[]);setTotal(result.total??result.sessions?.length??0);setLibraryReady(true);
      const current=navigationRef.current;
      if(!current.selectionExplicit){
        const nextSelected=current.selected&&result.sessions?.some(s=>String(s.id)===String(current.selected))?current.selected:result.sessions?.[0]?.id??null;
        if(nextSelected!==current.selected)navigate({selected:nextSelected},{replace:true});
      }
      setSyncError('');
      const lastPage=Math.max(0,Math.ceil((result.total??0)/PAGE_SIZE)-1);
      if(page>lastPage)setPage(lastPage);
    }catch(e){if(requestId===sessionRequest.current){if(syncActiveRef.current&&isSyncLockError(e))return;setSessions([]);setTotal(0);setSyncError(e.message);}}
    finally{if(requestId===sessionRequest.current)setLoading(false);}
  },[query,hostFilter,agentFilter,projectFilter,modelFilters,branchFilter,toolFilters,timeFilter,sortOrder,starredOnly,tagFilters,page,navigate]);

  useEffect(()=>{loadMeta();},[loadMeta]);
  useEffect(()=>{const timer=setInterval(()=>setCurrentTime(Date.now()),60000);return()=>clearInterval(timer);},[]);
  useEffect(()=>{setPage(0);},[query,hostFilter,agentFilter,projectFilter,modelFilters,branchFilter,toolFilters,timeFilter,sortOrder,starredOnly,tagFilters]);
  useEffect(()=>{const t=setTimeout(loadSessions,query?180:0);return()=>clearTimeout(t);},[loadSessions,query]);
  const loadDetail=useCallback(async({silent=false,page=detailPage}={})=>{
    const id=++detailRequest.current;
    if(!selected||view!=='sessions'){setDetail(null);setDetailError('');setDetailLoading(false);return;}
    if(!silent){setDetailLoading(true);setDetailError('');}
    try{
      const detailParams={id:String(selected),limit:String(DETAIL_PAGE_SIZE),page:String(page)};
      const data=await apiCall('get_session',detailParams,`${API}/sessions/${selected}?${new URLSearchParams({limit:detailParams.limit,page:detailParams.page})}`);
      if(id===detailRequest.current){setDetail(data);setDetailError('');}
    }
    catch(e){if(id===detailRequest.current&&!silent){setDetail(null);setDetailError(e.message||'Could not load this session.');}}
    finally{if(id===detailRequest.current)setDetailLoading(false);}
  },[selected,detailPage,view]);
  useEffect(()=>{loadDetail();return()=>{detailRequest.current++;};},[loadDetail]);
  useEffect(()=>{setCollapsedEntries(new Set());setDetailPage(1);setDetailJump('');setTagDraft('');},[selected]);
  useEffect(()=>{let cancelled=false;apiCall('get_sync_status',{},`${API}/sync/status`).then(data=>{if(!cancelled){if(!syncLaunchPending.current)setSync(data);setSyncStatusReady(true);}}).catch(()=>{});return()=>{cancelled=true;};},[]);
  useEffect(()=>{
    if(!sync.running)return;
    let active=true,busy=false,lastRefresh=0,lastMeta=0;
    const timer=setInterval(async()=>{
      if(busy)return;busy=true;
      try{
        const s=await apiCall('get_sync_status',{},`${API}/sync/status`);if(!active)return;setSync(s);
        const now=Date.now();
        if(!s.running){
          setSyncStopping(false);clearInterval(timer);setReportRevision(value=>value+1);loadMeta({silent:true});loadSessions({silent:true});loadDetail({silent:true});
          if(s.errors?.length){const hosts=s.errors.filter(e=>e.startsWith('host ')).map(e=>e.slice(5).split(':')[0]),files=s.errors.length-hosts.length;setNotice([hosts.length&&`Could not reach ${hosts.join(', ')}; showing sessions from the last successful fetch.`,files&&`${files} source files could not be read.`].filter(Boolean).join(' '));}
          else if(s.cancelled)setNotice('Sync paused safely. Run sync updates to continue.');
        }else{
          if(now-lastRefresh>=2500){lastRefresh=now;loadSessions({silent:true});loadDetail({silent:true});}
          if(now-lastMeta>=10000){lastMeta=now;loadMeta({silent:true});}
        }
      }catch{}finally{busy=false;}
    },500);
    return()=>{active=false;clearInterval(timer);};
  },[sync.running,loadMeta,loadSessions,loadDetail]);
  useEffect(()=>{if(!sortOpen)return;const h=e=>{if(sortAnchorRef.current&&!sortAnchorRef.current.contains(e.target))setSortOpen(false);};document.addEventListener('pointerdown',h);return()=>document.removeEventListener('pointerdown',h);},[sortOpen]);

  const refreshManagedData=async()=>{
    sessionRequest.current++;detailRequest.current++;
    setDetail(null);setDetailError('');setDetailPage(1);setPage(0);
    navigate({selected:null,selectionExplicit:false},{replace:true});
    setReportRevision(value=>value+1);
    await Promise.all([loadMeta({silent:true}),loadSessions({silent:true})]);
  };

  const updateAnnotations=async(patch)=>{
    if(!detail||annotationBusy)return;
    const id=detail.session.id;
    setAnnotationBusy(true);
    try{
      const updated=await apiCall('update_session_annotations',{id,...patch},null);
      setDetail(current=>current?.session.id===id?{...current,session:{...current.session,...updated}}:current);
      setTagDraft('');
      await Promise.all([loadSessions({silent:true}),loadMeta({silent:true})]);
    }catch(e){setNotice(e.message);}finally{setAnnotationBusy(false);}
  };
  const toggleSessionStar=async(session)=>{
    const id=session.id;
    if(starPendingRef.current.has(id))return;
    starPendingRef.current.add(id);setStarPending(new Set(starPendingRef.current));
    try{
      const updated=await apiCall('update_session_annotations',{id,starred:!session.starred},null);
      setSessions(current=>current.map(item=>item.id===id?{...item,...updated}:item));
      setDetail(current=>current?.session.id===id?{...current,session:{...current.session,...updated}}:current);
      if(starredOnly)await loadSessions({silent:true});
    }catch(e){setNotice(e.message);}finally{starPendingRef.current.delete(id);setStarPending(new Set(starPendingRef.current));}
  };
  const stopExport=async()=>{
    const job=exportJobRef.current;
    if(!job||job.cancelled||job.stopping||job.saved)return;
    job.stopping=true;setExportStopping(true);
    // During file saving, the backend decides whether cancellation arrived before commit.
    if(!job.saving){job.cancelled=true;job.worker?.terminate();job.reject?.(new Error('Export stopped'));}
    job.stopPromise=(async()=>{
      try{
        if(job.registered){const stopped=await apiCall('cancel_export',{jobId:job.id},null);if(stopped)job.cancelled=true;}
      }catch(e){if(!job.saved)setNotice(e.message);}
    })();
    await job.stopPromise;
  };
  const runExport=async()=>{
    if(exportJobRef.current)return;
    const job={id:crypto.randomUUID(),cancelled:false};
    exportJobRef.current=job;
    setExportBusy(true);setExportStopping(false);setExportProgress({phase:'Preparing export',done:0,total:0});
    let unlisten;
    try{
      const extension=exportFormat==='json'?'json':'md';
      await apiCall('begin_export',{jobId:job.id},null);job.registered=true;
      if(job.cancelled){await apiCall('cancel_export',{jobId:job.id},null);return;}
      unlisten=await listen('export-progress',({payload})=>{if(payload.jobId===job.id&&!job.cancelled)setExportProgress({phase:'Reading sessions',done:payload.done,total:payload.total});});
      if(job.cancelled)return;
      setExportProgress({phase:'Reading sessions',done:0,total:exportScope==='session'?1:total});
      const data=await apiCall('export_sessions',{
        jobId:job.id,scope:exportScope,id:exportScope==='session'?detail?.session.id:null,includeStats,
        filters:{q:query,host:hostFilter,agent:agentFilter,project:projectFilter,models:modelFilters,branch:branchFilter,tools:toolFilters,since:filterSince(timeFilter),sort:sortOrder,starred:starredOnly,tags:tagFilters},
      },null);
      if(job.cancelled)return;
      setExportProgress({phase:'Formatting export',done:0,total:0});
      const contents=await new Promise((resolve,reject)=>{
        job.reject=reject;
        job.worker=new Worker(new URL('./export-worker.js',import.meta.url),{type:'module'});
        job.worker.onmessage=({data})=>data.error?reject(new Error(data.error)):resolve(data.contents);
        job.worker.onerror=()=>reject(new Error('Could not format export'));
        job.worker.postMessage({data,format:exportFormat});
      });
      job.worker.terminate();job.reject=null;
      if(job.cancelled)return;
      setExportProgress({phase:'Choose where to save',done:0,total:0});
      const path=await save({defaultPath:exportFilename(exportScope,data.sessions[0]?.session,data.sessions.length,exportFormat),filters:[{name:exportFormat==='json'?'JSON':'Markdown',extensions:[extension]}]});
      if(!path||job.cancelled)return;
      setExportProgress({phase:'Saving file',done:0,total:0});
      job.saving=true;
      await apiCall('save_export',{jobId:job.id,path,contents},null);
      job.saved=true;
      await job.stopPromise;
      setExportOpen(false);setNotice(`Exported ${data.sessions.length} session${data.sessions.length===1?'':'s'}.`);
    }catch(e){await job.stopPromise;setNotice(job.cancelled?'Export stopped.':e.message);}finally{
      await job.stopPromise;
      job.worker?.terminate();unlisten?.();
      try{await apiCall('finish_export',{jobId:job.id},null);}catch{}
      if(job.cancelled&&!job.saved)setNotice('Export stopped.');
      exportJobRef.current=null;setExportBusy(false);setExportStopping(false);
    }
  };

  const startSync=useCallback(async({mode='incremental',agent=''}={})=>{
    if(sync.running||syncLaunchPending.current)return;
    startupSyncHandled.current=true;syncLaunchPending.current=true;
    setSyncStarting(true);setSyncStopping(false);setSyncMenuOpen(false);setSyncError('');
    setSync({running:true,startedAt:new Date().toISOString(),filesDone:0,filesTotal:0,mode,family:agent||null});
    try{const params=new URLSearchParams({mode});if(agent)params.set('agent',agent);const s=await apiCall('start_sync',{agent:agent||'',mode},`${API}/sync?${params}`,{method:'POST'});setSync({...s,startedAt:new Date().toISOString(),mode,family:agent||null});}
    catch(e){setSync({running:false});setSyncError(e.message);}
    finally{syncLaunchPending.current=false;setSyncStarting(false);}
  },[sync.running]);

  useEffect(()=>{
    if(startupSyncHandled.current||!hasTauriRuntime()||!libraryReady||!syncStatusReady||view!=='sessions'||loading||dataManagementOpen)return;
    if(!autoSync){startupSyncHandled.current=true;return;}
    if(sync.running||syncStarting){startupSyncHandled.current=true;return;}
    if(selected&&!detailError&&String(detail?.session.id)!==String(selected))return;
    // Two animation frames let the loaded library/transcript paint before scanning.
    let secondFrame;
    const firstFrame=requestAnimationFrame(()=>{secondFrame=requestAnimationFrame(()=>{
      if(!startupSyncHandled.current)startSync();
    });});
    return()=>{cancelAnimationFrame(firstFrame);if(secondFrame)cancelAnimationFrame(secondFrame);};
  },[autoSync,libraryReady,syncStatusReady,view,loading,dataManagementOpen,sync.running,syncStarting,selected,detail,detailError,startSync]);

  const stopSync=async()=>{
    if(!sync.running||syncStarting||syncStopping||sync.phase==='stopping')return;
    setSyncStopping(true);setSyncMenuOpen(false);setSyncError('');
    try{await apiCall('stop_sync',{},`${API}/sync/stop`,{method:'POST'});}
    catch(e){setSyncStopping(false);setSyncError(e.message);}
  };

  const groups=useMemo(()=>{
    const out=[];let key='';for(const s of sessions){const k=(parseDate(s.last_event_at||s.started_at)?.toDateString()||'Date unavailable');if(k!==key){key=k;out.push({label:relativeDate(s.last_event_at||s.started_at),items:[]});}out.at(-1).items.push(s);}return out;
  },[sessions]);
  const entryCounts=useMemo(()=>{
    const entries=detail?.entries||[];
    let user=0,assistant=0,tool=0,event=0;
    for(const e of entries){
      if(hasToolActivity(e))tool++;
      if(e.role==='user')user++;
      else if(e.role==='assistant')assistant++;
      if(isSessionEvent(e))event++;
    }
    return{all:entries.length,user,assistant,tool,event};
  },[detail]);
  const entryCountLabel=(type)=>`${entryCounts[type].toLocaleString()}/${(detail?.entryCounts?.[type]??entryCounts[type]).toLocaleString()}`;
  const detailEntries=useMemo(()=>{
    const entries=detail?.entries||[];
    return entries.filter(e=>{
      if(entryFilter==='user')return e.role==='user';
      if(entryFilter==='assistant')return e.role==='assistant';
      if(entryFilter==='tool')return hasToolActivity(e);
      if(entryFilter==='event')return isSessionEvent(e);
      return true;
    });
  },[detail,entryFilter]);
  // H6: transcript pagination derived from server total/limit/offset.
  const detailTotal=detail?.total??detail?.entries?.length??0;
  const detailLimit=detail?.limit||DETAIL_PAGE_SIZE;
  const detailTotalPages=Math.max(1,Math.ceil(detailTotal/detailLimit));
  const detailSafePage=Math.min(Math.max(1,detailPage),detailTotalPages);
  const detailRangeStart=detailTotal?((detailSafePage-1)*detailLimit)+1:0;
  const detailRangeEnd=detailTotal?Math.min(detailTotal,detailSafePage*detailLimit):0;
  const gotoDetailPage=(p)=>{const n=Math.min(Math.max(1,Math.floor(Number(p)||1)),detailTotalPages);setDetailPage(n);setDetailJump('');};
  const submitDetailJump=(e)=>{e?.preventDefault?.();if(detailJump.trim()==='')return;gotoDetailPage(Number(detailJump));};
  const isDetailLoading=loading||detailLoading||(Boolean(selected)&&!detail&&!detailError);
  const matchingProjects=projects.filter(p=>`${p.display_name||''} ${p.path||''}`.toLowerCase().includes(workspaceQuery.trim().toLowerCase()));
  const shownProjects=matchingProjects.slice(0,workspaceQuery||allWorkspaces?matchingProjects.length:9);
  const latestSync=syncCheckpoints.reduce((latest,item)=>{
    if(!item?.completed_at) return latest;
    if(!latest) return item.completed_at;
    const itemMs = parseDate(item.completed_at)?.getTime() || 0;
    const latestMs = parseDate(latest)?.getTime() || 0;
    return itemMs > latestMs ? item.completed_at : latest;
  },null);
  const checkpointFor=family=>syncCheckpoints.find(item=>item.family===family)?.completed_at;

  return <div className="app-shell" data-theme={theme}>
    {remoteHostsOpen&&<RemoteHosts onClose={()=>{setRemoteHostsOpen(false);loadMeta({silent:true});}} onSyncNow={()=>startSync()} syncRunning={sync.running||syncStarting}/>}
    {dataManagementOpen&&<DataManagement onClose={()=>setDataManagementOpen(false)} onChanged={refreshManagedData} syncRunning={sync.running||syncStarting}/>}
    <aside className={`sidebar ${mobileNav?'sidebar-open':''}`}>
      <div className="sidebar-brand"><div className="brand-row"><div className="brand-mark"><BrandMark size={19}/></div><span>{BRAND.wordmark[0]}<span className="brand-muted">{BRAND.wordmark[1]}</span></span><button className="icon-button sidebar-mobile-close" onClick={()=>setMobileNav(false)}><X size={17}/></button></div><span className="sidebar-publisher">{BRAND.tagline}</span></div>
      <div className="sidebar-scroll">
        <div className="nav-section agent-section"><div className="section-heading"><div className="section-label">AGENTS</div></div>
          <button className={`nav-item agent-nav ${view==='sessions'&&!agentFilter&&!projectFilter?'nav-active':''}`} onClick={()=>{navigate({view:'sessions',selectionExplicit:false});setAgentFilter('');setProjectFilter('');}}><span className="agent-mark gray"><Layers3 size={11}/></span><span>All</span><span className="nav-count">{metadataState.stats==='ready'?stats.sessions.toLocaleString():'—'}</span></button>
          {agents.map(a=><button key={a.family} className={`nav-item agent-nav ${view==='sessions'&&agentFilter===a.family?'nav-active':''}`} onClick={()=>{navigate({view:'sessions',selectionExplicit:false});setAgentFilter(agentFilter===a.family?'':a.family);setProjectFilter('');}}><span className={`agent-mark ${agentColors[a.family]||'gray'}`}>{agentMark[a.family]||a.product?.[0]}</span><span>{a.product}</span><span className="nav-count">{a.sessions.toLocaleString()}</span></button>)}
          {metadataStatus('agents','Loading agents…')}
          {metadataState.agents==='ready'&&!agents.length&&<div className="nav-empty">Sync to discover agents</div>}
        </div>
        <div className="nav-section host-section"><div className="section-heading"><div className="section-label">HOSTS</div><button className="tiny-icon" title="Manage remote hosts" aria-label="Manage remote hosts" onClick={()=>setRemoteHostsOpen(true)}><Server size={14}/></button></div>
          {(facets.hosts||[]).length>0?<>
            <button className={`nav-item agent-nav ${view==='sessions'&&hostFilter==='@local'?'nav-active':''}`} onClick={()=>{navigate({view:'sessions',selectionExplicit:false});setHostFilter(hostFilter==='@local'?'':'@local');setProjectFilter('');}}><Laptop size={15}/><span>This computer</span></button>
            {facets.hosts.map(h=><button key={h.value} title={`Sessions from ${h.value}`} className={`nav-item agent-nav ${view==='sessions'&&hostFilter===h.value?'nav-active':''}`} onClick={()=>{navigate({view:'sessions',selectionExplicit:false});setHostFilter(hostFilter===h.value?'':h.value);setProjectFilter('');}}><Server size={15}/><span className="ellipsis">{h.value}</span><span className="nav-count">{h.sessions.toLocaleString()}</span></button>)}
          </>:<button className="nav-item show-more" onClick={()=>setRemoteHostsOpen(true)}>Add SSH hosts <ArrowRight size={13}/></button>}
        </div>
        <div className="nav-section project-section"><div className="section-heading"><div className="section-label">WORKSPACES</div><div className="workspace-menu-anchor"><button className={`tiny-icon ${workspaceMenuOpen?'workspace-menu-active':''}`} title="Workspace options" aria-label="Workspace options" onClick={()=>setWorkspaceMenuOpen(!workspaceMenuOpen)}><MoreHorizontal size={16}/></button>{workspaceMenuOpen&&<div className="workspace-menu"><button onClick={()=>{setAllWorkspaces(!allWorkspaces);setWorkspaceMenuOpen(false);}}>{allWorkspaces?'Show fewer workspaces':'Show all workspaces'}</button><button disabled={!projectFilter} onClick={()=>{setProjectFilter('');setWorkspaceMenuOpen(false);}}>Clear selected workspace</button><button disabled={!workspaceQuery} onClick={()=>{setWorkspaceQuery('');setWorkspaceMenuOpen(false);}}>Clear workspace search</button></div>}</div></div>
          <label className="workspace-search"><Search size={13}/><input value={workspaceQuery} onChange={e=>setWorkspaceQuery(e.target.value)} disabled={metadataState.projects==='loading'&&!projects.length} placeholder={metadataState.projects==='loading'&&!projects.length?'Loading workspaces…':'Find a workspace…'} aria-label="Search workspaces"/>{workspaceQuery&&<button type="button" title="Clear workspace search" onClick={()=>setWorkspaceQuery('')}><X size={12}/></button>}</label>
          {shownProjects.map(p=><button key={p.path} title={p.path} className={`nav-item project-nav ${view==='sessions'&&projectFilter===p.path?'nav-active':''}`} onClick={()=>{navigate({view:'sessions',selectionExplicit:false});setProjectFilter(projectFilter===p.path?'':p.path);setAgentFilter('');}}><Folder size={15}/><span className="ellipsis">{p.display_name||shortPath(p.path)}</span><span className="nav-count">{p.sessions}</span></button>)}
          {metadataStatus('projects','Loading workspaces…')}
          {metadataState.projects==='ready'&&!shownProjects.length&&<div className="nav-empty">{workspaceQuery?'No matching workspaces':'No workspaces yet'}</div>}
          {matchingProjects.length>9&&!workspaceQuery&&<button className="nav-item show-more" onClick={()=>setAllWorkspaces(!allWorkspaces)}>{allWorkspaces?'Show fewer workspaces':'Show all workspaces'} <ArrowRight size={13}/></button>}
        </div>
      </div>
    </aside>

    <main className="main-panel">
      <header className="topbar"><div className="top-left"><button className="icon-button mobile-menu" onClick={()=>setMobileNav(true)}><Menu size={19}/></button><div className="breadcrumbs"><span>Library</span><span className="crumb-slash">/</span><strong>{view==='analytics'?'Analytics':agentFilter?familyLabel(agentFilter):projectFilter?shortPath(projectFilter):'All sessions'}</strong></div><span className="last-sync-label">Last sync: {metadataState['sync/checkpoints']==='loading'?<><LoaderCircle className="spin" size={11}/> Loading…</>:metadataState['sync/checkpoints']==='error'?'Unavailable':latestSync?formatSyncTime(latestSync,currentTime):'Never'}</span></div><div className="top-actions"><button className={`analytics-btn ${view!=='analytics'?'active':''}`} onClick={()=>setView('sessions')} title="View sessions" aria-label="View sessions" aria-pressed={view!=='analytics'}><MessageSquareText size={15}/><span>Sessions</span></button><button className={`analytics-btn ${view==='analytics'?'active':''}`} onClick={()=>setView('analytics')} title="View analytics" aria-label="View analytics" aria-pressed={view==='analytics'}><TrendingUp size={15}/><span>Analytics</span></button><div className="sync-action"><div className="sync-action-row"><button type="button" className={`sync-button ${sync.running?'sync-stop':''}`} onClick={sync.running?stopSync:()=>{setSyncMenuOpen(!syncMenuOpen);}} disabled={syncStarting||syncStopping||(sync.running&&sync.phase==='stopping')} aria-label={sync.running?'Stop session sync':'Sync options'} aria-haspopup={sync.running?undefined:'dialog'} aria-expanded={sync.running?undefined:syncMenuOpen} title={sync.running?'Stop session sync':'Sync options'}>{sync.running?<Square size={14}/>:<RefreshCw size={15}/>}<span>{syncStarting?'Starting…':syncStopping||sync.phase==='stopping'?'Stopping…':sync.running?'Stop sync':'Sync updates'}</span>{sync.running&&<span className="button-count">{sync.filesDone||0}/{sync.filesTotal||0}</span>}{!sync.running&&<ChevronDown size={14}/>}</button>{sync.running&&<button type="button" className="sync-options-button" onClick={()=>setSyncMenuOpen(!syncMenuOpen)} aria-label="Sync options" title="Sync options" aria-haspopup="dialog" aria-expanded={syncMenuOpen}><ChevronDown size={14}/></button>}</div>{syncMenuOpen&&<SyncMenu
          agents={agents} disabled={sync.running||syncStarting} autoSync={autoSync}
          toggleAutoSync={toggleAutoSync} startSync={startSync} onClose={()=>setSyncMenuOpen(false)}
          agentStatus={metadataStatus('agents','Loading agents…')}
          checkpointStatus={metadataStatus('sync/checkpoints','Loading sync history…')}
          agentDescription={family=>metadataState['sync/checkpoints']==='loading'?'Loading sync history…':metadataState['sync/checkpoints']==='error'?'Sync history unavailable':checkpointFor(family)?`Since ${formatSyncTime(checkpointFor(family),currentTime)}`:'First sync · scans all history'}
        />}</div><button type="button" className="theme-toggle" onClick={()=>{setSyncMenuOpen(false);setDataManagementOpen(true);}} title="Manage saved data" aria-label="Manage saved data" aria-haspopup="dialog"><Database size={16}/></button><button className="theme-toggle" onClick={toggleTheme} title={`Switch to ${theme==='dark'?'light':'dark'} theme`} aria-label={`Switch to ${theme==='dark'?'light':'dark'} theme`} aria-pressed={theme==='light'}>{theme==='dark'?<Sun size={16}/>:<Moon size={16}/>}</button></div></header>
      {sync.running&&<div className="sync-progress"><span style={{width:`${sync.filesTotal?Math.min(100,sync.filesDone/sync.filesTotal*100):2}%`}}/></div>}
      {view === 'analytics' ? (
        <AnalyticsView
          theme={theme}
          agents={agents}
          projects={projects}
          metadataState={metadataState}
          refreshKey={reportRevision}
          syncRunning={sync.running}
          navigationState={navigation.analytics}
          onNavigationChange={updateAnalyticsNavigation}
          onSelectSession={selectSession}
          onClose={() => setView('sessions')}
        />
      ) : (
        <div className="content-area">
          <div className="page-intro"><div><h1>{agentFilter?familyLabel(agentFilter):projectFilter?shortPath(projectFilter):'Sessions'}</h1></div><div className="intro-stats"><div><strong>{metadataState.stats==='ready'?stats.sessions.toLocaleString():'—'}</strong><span>sessions</span></div><i/><div><strong>{metadataState.stats==='ready'?(stats.agents||0):'—'}</strong><span>agents</span></div><i/><div><strong>{metadataState.stats==='ready'?stats.files.toLocaleString():'—'}</strong><span>files seen</span></div></div></div>

          {notice&&<div className="notice"><Check size={15}/>{notice}<button onClick={()=>setNotice('')}><X size={14}/></button></div>}
          <div className="toolbar"><div className="search-wrap"><Search size={16}/><input value={query} onChange={e=>setQuery(e.target.value)} placeholder="Search sessions…" aria-label="Search sessions"/>{query&&<button className="clear-search" onClick={()=>setQuery('')}><X size={14}/></button>}</div><div className="toolbar-actions"><button type="button" className={`sort-button ${starredOnly?'annotation-active':''}`} onClick={()=>setStarredOnly(v=>!v)} aria-pressed={starredOnly} title="Show starred sessions"><Star size={14} fill={starredOnly?'currentColor':'none'}/><span>Starred</span></button><DateFilter value={timeFilter} onChange={setTimeFilter}/><ModelFilter loading={metadataState.facets==='loading'} error={metadataState.facets==='error'} onRetry={loadMeta} models={facets.models} selected={modelFilters} onChange={setModelFilters}/><BranchFilter loading={metadataState.facets==='loading'} error={metadataState.facets==='error'} onRetry={loadMeta} branches={facets.branches} selected={branchFilter} onChange={setBranchFilter}/><ToolFilter loading={metadataState.facets==='loading'} error={metadataState.facets==='error'} onRetry={loadMeta} tools={facets.tools} selected={toolFilters} onChange={setToolFilters}/>{(timeFilter||modelFilters.length>0||branchFilter||toolFilters.length>0||starredOnly||tagFilters.length>0)&&<button type="button" className="clear-all-filters-btn" onClick={()=>{setTimeFilter('');setModelFilters([]);setBranchFilter('');setToolFilters([]);setStarredOnly(false);setTagFilters([]);}} title="Clear all active filters"><X size={12}/><span>Clear</span></button>}<div className="sort-anchor" ref={sortAnchorRef}><button className={`sort-button ${sortOpen?'sort-open':''}`} onClick={()=>setSortOpen(!sortOpen)} title="Sort sessions" aria-label="Sort sessions" aria-expanded={sortOpen}><ArrowUpDown size={14}/><span>Sort</span><ChevronDown size={13}/></button>{sortOpen&&<div className="sort-menu">{[{value:'recent',label:'Most recent'},{value:'oldest',label:'Oldest'},{value:'longest',label:'Longest'},{value:'messages',label:'Most messages'}].map(opt=><button key={opt.value} className={`sort-menu-item ${sortOrder===opt.value?'sort-menu-active':''}`} onClick={()=>{setSortOrder(opt.value);setSortOpen(false);}}><span>{opt.label}</span>{sortOrder===opt.value&&<Check size={13}/>}</button>)}</div>}</div></div></div>
          <div className="metadata-indicators">{metadataStatus('stats','Loading library counts…')}{metadataStatus('facets','Loading filters…')}</div>
          {syncError&&<div className="error-strip"><CircleHelp size={15}/><span>{syncError}</span><button onClick={()=>setSyncError('')}><X size={14}/></button></div>}

          <div className="browser-grid">
            <section className="session-column"><div className="list-head"><div><strong>{query?`${total} results`:`${total.toLocaleString()} sessions`}</strong><span>{query?'matching your search':`sorted by ${{recent:'most recent',oldest:'oldest',longest:'longest',messages:'most messages'}[sortOrder]}`}</span></div><button type="button" className="sort-button" title="Export sessions" aria-label="Export sessions" onClick={()=>setExportOpen(true)}><Download size={14}/><span>Export</span></button></div>
              <div className="session-list">
                {loading&&<div className="session-loading" role="status" aria-live="polite"><div className="loading-label"><LoaderCircle className="spin" size={15}/><span>Loading {agentFilter?`${familyLabel(agentFilter)} sessions`:'sessions'}</span></div>{[0,1,2].map(item=><div className="session-skeleton" key={item}><span className="skeleton-line skeleton-agent"/><span className="skeleton-line skeleton-title"/><span className="skeleton-line skeleton-copy"/><span className="skeleton-line skeleton-meta"/></div>)}</div>}
                {!loading&&groups.map(g=><div className="date-group" key={g.label}><div className="date-label">{g.label}</div>{g.items.map(s=><div key={s.id} className="session-card-wrap"><button type="button" className={`session-card ${String(selected)===String(s.id)?'session-selected':''}`} onClick={()=>selectSession(s.id)}>
                  <div className="card-top"><span className={`agent-mark ${agentColors[s.agent]||'gray'}`}>{agentMark[s.agent]||s.product?.[0]}</span><span className="card-agent">{s.product||familyLabel(s.agent)}</span><span className="card-time">{parseDate(s.last_event_at||s.started_at)?.toLocaleTimeString([],{hour:'numeric',minute:'2-digit'})||'—'}</span></div>
                  <strong className="session-title">{s.title||'Untitled session'}</strong>{s.tags?.length>0&&<div className="card-tags">{s.tags.map(tag=><span className="tag-chip" key={tag}>{tag}</span>)}</div>}<p className="session-preview">{(s.preview||'No text preview available').replace(/\s+/g,' ').slice(0,145)}</p>
                  <div className="card-meta">{s.host&&<span className="host-chip" title={`Recorded on ${s.host}`}><Server size={11}/>{s.host}</span>}<span><Folder size={12}/>{shortPath(s.project||s.cwd)}</span>{s.branch&&<span><GitBranch size={12}/>{s.branch}</span>}</div>
                </button><button type="button" className={`card-star ${s.starred?'is-starred':''}`} aria-label={`${s.starred?'Unstar':'Star'} ${s.title||'session'}`} aria-pressed={Boolean(s.starred)} title={s.starred?'Unstar session':'Star session'} disabled={starPending.has(s.id)} onClick={e=>{e.stopPropagation();toggleSessionStar(s);}}><Star size={15} fill={s.starred?'currentColor':'none'}/></button>                </div>)}</div>)}
                {!loading&&!sessions.length&&<div className="empty-list"><div className="empty-orb"><MessageSquareText size={20}/></div><strong>{query?'No matching sessions':'Your session library is waiting'}</strong><p>{query?'Try a different phrase or clear a filter.':'Sync local agent history to bring your conversations together.'}</p>{!query&&<button className={`sync-button ${sync.running?'sync-stop':''}`} disabled={syncStarting||syncStopping||(sync.running&&sync.phase==='stopping')} onClick={sync.running?stopSync:()=>startSync()}>{sync.running?<Square size={14}/>:<RefreshCw size={14}/>}<span>{syncStarting?'Starting…':syncStopping||sync.phase==='stopping'?'Stopping…':sync.running?'Stop sync':'Sync your sessions'}</span></button>}</div>}
              </div>
              <div className="list-footer"><span>Showing {total?`${(page*PAGE_SIZE)+1}–${(page*PAGE_SIZE)+sessions.length} of ${total.toLocaleString()}`:'0 of 0'}</span><div className="pagination-controls"><button title="Previous page" aria-label="Previous page" disabled={page===0||loading} onClick={()=>setPage(current=>Math.max(0,current-1))}><ChevronLeft size={13}/></button><span>{total?`${page+1} / ${Math.ceil(total/PAGE_SIZE)}`:'0 / 0'}</span><button title="Next page" aria-label="Next page" disabled={(page+1)*PAGE_SIZE>=total||loading} onClick={()=>setPage(current=>current+1)}><ChevronRight size={13}/></button></div><button title="Refresh" aria-label="Refresh sessions" onClick={loadSessions}><RefreshCw size={13}/></button></div>
            </section>

            <section className="detail-column">
              {detail&&!isDetailLoading?<>
                <div className="detail-header"><div className="detail-title-wrap"><div className="detail-agent-line"><span className={`agent-mark ${agentColors[detail.session.agent]||'gray'}`}>{agentMark[detail.session.agent]||detail.session.product?.[0]}</span><span>{detail.session.product||familyLabel(detail.session.agent)}</span><span className="detail-sep">·</span><span>{relativeDate(detail.session.last_event_at||detail.session.started_at)}</span></div><h2>{detail.session.title||'Untitled session'}</h2><div className="detail-subline">{detail.session.host&&<span className="host-chip"><Server size={12}/>{detail.session.host}</span>}<span><Folder size={13}/>{detail.session.project?shortPath(detail.session.project):shortPath(detail.session.cwd)}</span>{detail.session.branch&&<span><GitBranch size={13}/>{detail.session.branch}</span>}{detail.session.model&&<span><Sparkles size={13}/>{detail.session.model}</span>}</div></div><div className="detail-header-actions"><button type="button" className="resume-button" title={detail.session.host?`Open the workspace on ${detail.session.host} with VS Code Remote-SSH`:'Open the workspace in VS Code'} aria-label="Open workspace in VS Code" onClick={openInEditor}><Code2 size={14}/><span>VS Code</span></button><button type="button" className="resume-button" title="Resume this session in its native agent CLI" aria-label="Resume this session" onClick={openResume}><Terminal size={14}/><span>Resume</span></button></div></div>
                <div className="session-annotations">{(detail.session.tags||[]).map(tag=><button type="button" className="tag-chip" key={tag} disabled={annotationBusy} title={`Remove tag ${tag}`} aria-label={`Remove tag ${tag}`} onClick={()=>updateAnnotations({tags:detail.session.tags.filter(t=>t!==tag)})}>{tag}<X size={11}/></button>)}<form className="tag-form" onSubmit={e=>{e.preventDefault();if(tagDraft.trim())updateAnnotations({tags:[...(detail.session.tags||[]),tagDraft.trim()]});}}><input value={tagDraft} onChange={e=>setTagDraft(e.target.value)} maxLength={80} placeholder="Add a tag…" aria-label="New session tag" disabled={annotationBusy}/><button type="submit" disabled={annotationBusy||!tagDraft.trim()}>Add</button></form><button type="button" className="icon-button" title="Export this session" aria-label="Export this session" onClick={()=>{setExportScope('session');setExportOpen(true);}}><Download size={15}/></button></div>
                <div className="transcript-toolbar">
                  <nav className="transcript-pagination" aria-label="Transcript pages">
                    <div className="pagination-controls">
                      <button title="First page" aria-label="First transcript page" disabled={detailSafePage<=1||isDetailLoading} onClick={()=>gotoDetailPage(1)}><ChevronLeft size={13}/><ChevronLeft size={13} style={{marginLeft:-10}} /></button>
                      <button title="Previous page" aria-label="Previous transcript page" disabled={detailSafePage<=1||isDetailLoading} onClick={()=>gotoDetailPage(detailSafePage-1)}><ChevronLeft size={13}/></button>
                      <form className="pagination-jump" onSubmit={submitDetailJump}>
                        <span aria-live="polite">{detailTotal?`${detailRangeStart}–${detailRangeEnd} of ${detailTotal.toLocaleString()}`:'0 of 0'} · Page <input value={detailJump} onChange={e=>setDetailJump(e.target.value.replace(/[^0-9]/g,''))} placeholder={String(detailSafePage)} aria-label={`Go to transcript page, current page ${detailSafePage} of ${detailTotalPages}`} title="Enter a page number and press Enter" inputMode="numeric" disabled={isDetailLoading} /> / {detailTotalPages}</span>
                      </form>
                      <button title="Next page" aria-label="Next transcript page" disabled={detailSafePage>=detailTotalPages||isDetailLoading} onClick={()=>gotoDetailPage(detailSafePage+1)}><ChevronRight size={13}/></button>
                      <button title="Last page" aria-label="Last transcript page" disabled={detailSafePage>=detailTotalPages||isDetailLoading} onClick={()=>gotoDetailPage(detailTotalPages)}><ChevronRight size={13}/><ChevronRight size={13} style={{marginLeft:-10}} /></button>
                    </div>
                  </nav>
                  <div className="transcript-filter-actions">
                    <div className="transcript-filters" role="group" aria-label="Filter transcript entries">
                      <button className={`transcript-toggle ${entryFilter==='all'?'active':''}`} onClick={()=>setEntryFilter('all')} title="Show all entries">All<b>{entryCountLabel('all')}</b></button>
                      <button className={`transcript-toggle ${entryFilter==='user'?'active':''}`} onClick={()=>setEntryFilter(c=>c==='user'?'all':'user')} title="Filter by user messages"><User size={12}/>User<b>{entryCountLabel('user')}</b></button>
                      <button className={`transcript-toggle ${entryFilter==='assistant'?'active':''}`} onClick={()=>setEntryFilter(c=>c==='assistant'?'all':'assistant')} title="Filter by agent responses"><Bot size={12}/>Agent<b>{entryCountLabel('assistant')}</b></button>
                      <button className={`transcript-toggle ${entryFilter==='tool'?'active':''}`} onClick={()=>setEntryFilter(c=>c==='tool'?'all':'tool')} title="Filter by tool calls and results"><Terminal size={12}/>Tools<b>{entryCountLabel('tool')}</b></button>
                      <button className={`transcript-toggle ${entryFilter==='event'?'active':''}`} onClick={()=>setEntryFilter(c=>c==='event'?'all':'event')} title="Filter by session events"><Layers3 size={12}/>Events<b>{entryCountLabel('event')}</b></button>
                    </div>
                  </div>
                </div>
                <div className="transcript-scroll">
                  {detailEntries.map((e,i)=>{
                    const role=e.role||'event';const isAssistant=role==='assistant';const isUser=role==='user';
                    const {tools:toolsVisible,text:textParts,special}=transcriptParts(e);
                    const isCollapsed=collapsedEntries.has(e.id);
                    const firstText=textParts[0]?.text?.replace(/\s+/g,' ').trim();
                    return <article className={`transcript-entry ${isUser?'user-entry':isAssistant?'assistant-entry':'system-entry'}`} key={e.id}>
                      <div className="message-avatar">{isUser?<span>Y</span>:isAssistant?<Bot size={15}/>:<Code2 size={14}/>}</div>
                      <div className="message-content">
                        <div
                          className={`message-head ${isCollapsed?'is-collapsed':''}`}
                          onClick={()=>toggleCollapse(e.id)}
                          role="button"
                          tabIndex={0}
                          aria-expanded={!isCollapsed}
                          title={isCollapsed?'Click to expand message':'Click to collapse message'}
                          onKeyDown={ev=>{if(ev.key==='Enter'||ev.key===' '){ev.preventDefault();toggleCollapse(e.id);}}}
                        >
                          <ChevronDown size={12} className={`message-collapse-chevron ${isCollapsed?'collapsed':''}`}/>
                          <strong>{isUser?'You':isAssistant?(detail.session.product||'Assistant'):role==='tool'?'Tool':'Session event'}</strong>
                          <span>{e.occurred_at?parseDate(e.occurred_at)?.toLocaleTimeString([],{hour:'numeric',minute:'2-digit'}):e.kind?.replaceAll('_',' ')}</span>
                          {!isUser&&!isAssistant&&<span className="event-kind">{e.kind?.replaceAll('_',' ')}</span>}
                          {isCollapsed&&<span className="collapsed-preview">{firstText?(firstText.length>55?`${firstText.slice(0,52)}…`:firstText):special[0]?.text||toolsVisible[0]?.tool_name||'Collapsed'}</span>}
                          <button className="copy-entry" onClick={ev=>{ev.stopPropagation();copyEntry(e);}} title="Copy message as Markdown" aria-label="Copy message as Markdown">{copiedEntry===e.id?<Check size={13}/>:<Copy size={13}/>}<span>{copiedEntry===e.id?'Copied':'Copy MD'}</span></button>
                        </div>
                        {!isCollapsed&&<>
                          {textParts.map((p,j)=><div className="message-text markdown-body" key={p.id||j}><ReactMarkdown remarkPlugins={[remarkGfm]}>{p.text}</ReactMarkdown></div>)}
                          {special.map((p,j)=><div className="tool-block" key={p.id||j}><div className="tool-block-head"><Terminal size={13}/><strong>{p.kind==='tool_result'?'Tool result':p.text||p.kind.replaceAll('_',' ')}</strong><ChevronDown size={13}/></div><pre>{p.kind==='tool_result'&&p.text?p.text:p.data_json?formatData(p.data_json):p.text||'Tool activity recorded'}</pre></div>)}
                          {toolsVisible.map(t=><div className="tool-block" key={t.id}><div className="tool-block-head"><Terminal size={13}/><strong>{t.tool_name}</strong><span className="tool-status">{t.status||'invoked'}</span><ChevronDown size={13}/></div>{t.command_text&&<pre className="command-pre">$ {t.command_text}</pre>}{t.arguments_json&&<pre>{formatData(t.arguments_json)}</pre>}{t.result_text&&<pre>{t.result_text.slice(0,6000)}</pre>}</div>)}
                          {!textParts.length&&!special.length&&!toolsVisible.length&&role==='event'&&<div className="event-note">{e.kind?.replaceAll('_',' ')}<span>{e.payload_json&&formatData(e.payload_json)}</span></div>}
                        </>}
                      </div>
                    </article>;
                  })}
                  {!detailEntries.length&&<div className="transcript-empty"><div className="empty-orb"><FileCode2 size={20}/></div><strong>No {entryFilter==='all'?'displayable':entryFilter==='assistant'?'agent response':entryFilter} entries on this page</strong><span>{entryFilter!=='all'?'Try clearing the filter or selecting All.':'Session metadata is still available above.'}</span></div>}
                  <div className="transcript-end"><span/>{detailSafePage>=detailTotalPages?'END OF SESSION':`PAGE ${detailSafePage} OF ${detailTotalPages}`}<span/></div>
                </div>
              </>:isDetailLoading?<div className="detail-loading" role="status" aria-live="polite"><div className="loading-label"><LoaderCircle className="spin" size={16}/><span>Loading session…</span></div></div>:<div className="detail-placeholder"><div className="placeholder-art"><div className="art-ring ring-a"/><div className="art-ring ring-b"/><div className="art-center"><MessageSquareText size={25}/></div><span className="art-star star-one">✦</span><span className="art-star star-two">✧</span></div><h2>{detailError?'Could not load this session':'No session selected'}</h2><p>{detailError||'Select a session to explore the full conversation.'}</p></div>}
            </section>
          </div>
        </div>
      )}
    </main>
    {exportOpen&&<div className="export-backdrop" onClick={e=>{if(e.target===e.currentTarget&&!exportBusy)setExportOpen(false);}}><div ref={exportDialogRef} className="export-dialog" role="dialog" aria-modal="true" aria-labelledby="export-title" onKeyDown={e=>{if(e.key==='Escape'&&!exportBusy)setExportOpen(false);if(e.key==='Tab'){const controls=Array.from(e.currentTarget.querySelectorAll('button:not(:disabled),input:not(:disabled)'));const first=controls[0],last=controls.at(-1);if(e.shiftKey&&document.activeElement===first){e.preventDefault();last?.focus();}else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first?.focus();}}}}><div className="export-heading"><h2 id="export-title">Export sessions</h2><button type="button" className="icon-button" aria-label="Close export" disabled={exportBusy} onClick={()=>setExportOpen(false)}><X size={17}/></button></div><ExportSelect label="Export scope" value={exportScope} disabled={exportBusy} onChange={setExportScope} options={[{value:'session',label:'Selected session',disabled:!detail||isDetailLoading},{value:'filtered',label:`Filtered view (all ${total.toLocaleString()} matches)`},{value:'library',label:'Whole library'}]}/><ExportSelect label="Format" value={exportFormat} disabled={exportBusy} onChange={setExportFormat} options={[{value:'markdown',label:'Markdown (.md)'},{value:'json',label:'JSON (.json)'}]}/><label className="export-stats"><input type="checkbox" checked={includeStats} disabled={exportBusy} onChange={e=>{setIncludeStats(e.target.checked);localStorage.setItem(storageKey('export-stats'),String(e.target.checked));}}/>Include session stats (tokens, estimated cost, duration, turns, tools)</label><p>Exports include full transcripts, bookmarks, and tags. The stats preference is saved for future exports.</p>{exportBusy&&<div className="export-progress-status" role="status" aria-live="polite"><LoaderCircle className="spin" size={16}/><span>{exportStopping?'Stopping export…':exportProgress.phase}{!exportStopping&&exportProgress.total>0?` · ${exportProgress.done} / ${exportProgress.total}`:''}</span></div>}{exportBusy&&exportProgress.total>0&&<progress className="export-progress-bar" value={exportProgress.done} max={exportProgress.total} aria-label="Export progress"/>}<div className="export-actions">{exportBusy&&<button type="button" className="sort-button export-stop" disabled={exportStopping} onClick={stopExport}><X size={14}/>{exportStopping?'Stopping…':'Stop export'}</button>}<button type="button" className="sync-button" disabled={exportBusy||(exportScope==='session'&&(!detail||isDetailLoading))} onClick={runExport}>{exportBusy?<LoaderCircle className="spin" size={14}/>:<Download size={14}/>} {exportBusy?'Exporting…':'Save export'}</button></div></div></div>}
    {resumeOpen&&<div className="export-backdrop" onClick={e=>{if(e.target===e.currentTarget&&!resumeBusy)setResumeOpen(false);}}><div ref={resumeDialogRef} className="export-dialog resume-dialog" role="dialog" aria-modal="true" aria-labelledby="resume-title" onKeyDown={e=>{if(e.key==='Escape'&&!resumeBusy)setResumeOpen(false);}}><div className="export-heading"><h2 id="resume-title">Resume session</h2><button type="button" className="icon-button" aria-label="Close resume" disabled={resumeBusy} onClick={()=>setResumeOpen(false)}><X size={17}/></button></div>{detail&&<p className="resume-sub">{familyLabel(detail.session.agent)} · {detail.session.title||'Untitled session'}</p>}{resumeTerminals.length>0&&<ExportSelect label="Terminal" value={resumeTerminal} disabled={resumeBusy} onChange={changeResumeTerminal} options={resumeTerminals.map(t=>({value:t.id,label:t.available?t.label:`${t.label} — not installed`,disabled:!t.available}))}/>}<p className="resume-meta">Your choice is saved automatically and reused next time.</p>{resumeBusy&&!resumeInfo&&<div className="export-progress-status" role="status" aria-live="polite"><LoaderCircle className="spin" size={16}/><span>Preparing resume command…</span></div>}{resumeInfo?.displayCommand&&<pre className="resume-command">{resumeInfo.displayCommand}</pre>}{resumeInfo?.host?<p className="resume-meta">Runs on {resumeInfo.host} over SSH, in {detail?.session.cwd||'the home directory'}.</p>:resumeInfo?.cwd&&<p className="resume-meta">Working directory: {resumeInfo.cwd}{resumeInfo.cwdFallback?' (fallback)':''}</p>}{resumeInfo?.terminal&&<p className="resume-meta">Terminal: {resumeInfo.terminal}</p>}{resumeInfo?.cliFound===false&&<p className="resume-meta resume-warn">Agent CLI not found. Install it or copy the command above into a terminal.</p>}{resumeInfo?.sourceExists===false&&<p className="resume-meta resume-warn">Original session file is missing; the agent may report it as not found.</p>}{(resumeInfo?.warnings||[]).map(w=><p key={w} className="resume-meta resume-warn">{w}</p>)}{resumeError&&<div className="error-strip"><CircleHelp size={15}/><span>{resumeError}</span><button onClick={()=>setResumeError('')}><X size={14}/></button></div>}<div className="export-actions"><button type="button" className="sort-button" disabled={resumeBusy||!(resumeInfo?.displayCommand)} onClick={copyResumeCommand}>{resumeCopied?<Check size={14}/>:<Copy size={14}/>}{resumeCopied?'Copied':'Copy command'}</button><button type="button" className="sync-button" disabled={resumeBusy||!resumeInfo} onClick={launchResume}>{resumeBusy?<LoaderCircle className="spin" size={14}/>:<Terminal size={14}/>}{resumeBusy?'Working…':'Resume in terminal'}</button></div></div></div>}
  </div>;
}

function formatData(value){try{return JSON.stringify(JSON.parse(value),null,2).slice(0,6000);}catch{return String(value).slice(0,6000);}}

function entryAsMarkdown(entry){
  const sections=(entry.parts||[]).map(part=>{
    if(part.kind==='text')return part.text||'';
    if(part.kind==='tool_result')return part.text?`**Tool result**\n\n\`\`\`text\n${part.text}\n\`\`\``:'';
    const heading=part.text||part.kind?.replaceAll('_',' ')||'Session event';
    const body=part.data_json?formatData(part.data_json):'';
    return `**${heading}**${body?`\n\n\`\`\`json\n${body}\n\`\`\``:''}`;
  }).filter(Boolean);
  for(const tool of entry.tools||[]){
    sections.push(`**${tool.tool_name||'Tool'}**${tool.status?` · ${tool.status}`:''}`);
    if(tool.command_text)sections.push(`\`\`\`sh\n${tool.command_text}\n\`\`\``);
    if(tool.arguments_json)sections.push(`\`\`\`json\n${formatData(tool.arguments_json)}\n\`\`\``);
    if(tool.result_text)sections.push(`\`\`\`text\n${tool.result_text}\n\`\`\``);
  }
  if(!sections.length&&entry.payload_json)sections.push(`\`\`\`json\n${formatData(entry.payload_json)}\n\`\`\``);
  return sections.join('\n\n')||`_${entry.kind?.replaceAll('_',' ')||'Session event'}_`;
}

class ErrorBoundary extends React.Component {
  constructor(props){super(props);this.state={error:null};}
  static getDerivedStateFromError(error){return {error};}
  componentDidCatch(error,info){window.dispatchEvent(new Event(READY_EVENT));console.error('[boot] React error:',error,info);}
  render(){
    if(this.state.error){
      return <div style={{padding:20,color:'#eee',fontFamily:'sans-serif'}}><h2>{BRAND.name} failed to start</h2><pre style={{whiteSpace:'pre-wrap'}}>{String(this.state.error?.message||this.state.error)}</pre><button onClick={()=>window.location.reload()}>Reload</button></div>;
    }
    return this.props.children;
  }
}

console.log('[boot] hasTauriRuntime=',hasTauriRuntime());
window.onerror=function(message,source,lineno,colno,error){
  console.error('[boot] window.onerror:',message,source,lineno,colno,error);
  let el=document.getElementById('boot-error-overlay');
  if(!el){
    el=document.createElement('div');
    el.id='boot-error-overlay';
    el.style.cssText='position:fixed;left:0;right:0;bottom:0;z-index:9999;padding:8px 12px;background:#7a1f1f;color:#fff;font:12px monospace;white-space:pre-wrap';
    document.body.appendChild(el);
  }
  el.textContent='Error: '+(error?.stack||error?.message||message)+' @'+source+':'+lineno+':'+colno;
  return false;
};

createRoot(document.getElementById('root')).render(<React.StrictMode><ErrorBoundary><App/></ErrorBoundary></React.StrictMode>);
