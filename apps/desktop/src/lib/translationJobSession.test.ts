import assert from "node:assert/strict";
import { registerHooks } from "node:module";

// Exercise the real persistent session across terminal/cancel races. Only its
// network and UI sinks are replaced; lifecycle decisions stay in production code.
const h = {
  snapshot: null as any, jobId: null as string | null, translating: false,
  handlers: null as any, toasts: [] as any[], logs: [] as any[], unsubscribed: 0,
  cancellations: [] as string[],
};
(globalThis as any).__sessionHarness = h;
registerHooks({
  load(url, context, nextLoad) {
    const p = url.replace(/\\/g, "/");
    const modules: Record<string, string> = {
      "/src/lib/api.ts": `export async function cancelTranslation(id){globalThis.__sessionHarness.cancellations.push(id);}`,
      "/src/lib/ws.ts": `export const JOB_STREAM_LOST_MESSAGE='stream lost'; export function subscribeToJob(id,handlers){globalThis.__sessionHarness.handlers=handlers;return()=>{globalThis.__sessionHarness.unsubscribed++;};}`,
      "/src/lib/i18n/index.ts": `export function t(key){return key;}`,
      "/src/stores/editorStore.ts": `const h=globalThis.__sessionHarness; export const useEditorStore={getState:()=>({jobId:h.jobId,isTranslating:h.translating,jobSnapshot:h.snapshot,setJob:id=>{h.jobId=id;},setTranslating:v=>{h.translating=v;},setJobSnapshot:v=>{h.snapshot=v;},patchJobSnapshot:v=>{h.snapshot={...h.snapshot,...v};}})};`,
      "/src/stores/queueStore.ts": `export const useQueueStore={getState:()=>({setGlobalProgress:()=>{},globalProgress:null})};`,
      "/src/stores/logStore.ts": `export function addLog(...args){globalThis.__sessionHarness.logs.push(args);}`,
      "/src/stores/toastStore.ts": `export function addToast(...args){globalThis.__sessionHarness.toasts.push(args);}`,
    };
    const key = Object.keys(modules).find(suffix => p.endsWith(suffix));
    return key ? { format: "module", source: modules[key], shortCircuit: true } : nextLoad(url, context);
  },
});
const session = await import("./translationJobSession.ts");
session.setTranslationModalOpen(true);
const attach = (jobId: string) => session.attachTranslationJob({jobId,projectName:"Fixture",providerLabel:"Mock"});
attach("cancel-replayed");
h.handlers.onFailed({type:"failed",entry_id:null,error:"cancelled"});
assert.equal(h.snapshot.cancelled,true);
assert.equal(h.snapshot.error,null);
assert.equal(h.translating,false);
assert.equal(h.jobId,null);
assert.equal(h.toasts.at(-1)[0],"info");
assert.equal(h.toasts.at(-1)[1],"translate.toast.cancelled");
const count=h.toasts.length;
h.handlers.onClosed();
assert.equal(h.toasts.length,count,"socket close cannot finish cancellation twice");

attach("real-failure");
h.handlers.onFailed({type:"failed",entry_id:null,error:"authentication failed"});
assert.equal(h.snapshot.cancelled,false);
assert.equal(h.snapshot.error,"authentication failed");
assert.equal(h.toasts.at(-1)[0],"error");

attach("completion-wins");
await session.requestTranslationCancel();
assert.equal(h.snapshot.cancelling,true);
assert.equal(h.cancellations.at(-1),"completion-wins");
h.handlers.onCompleted({type:"completed",total_translated:3,total_cost:0,cost_is_complete:true});
assert.equal(h.snapshot.done,true);
assert.equal(h.snapshot.cancelled,false);
assert.equal(h.snapshot.cancelling,false);
assert.equal(h.toasts.at(-1)[0],"success");

attach("old-server-close");
await session.requestTranslationCancel();
h.handlers.onClosed();
assert.equal(h.snapshot.cancelled,true);
assert.equal(h.snapshot.error,null);
assert.equal(session.subscribedTranslationJobId(),null);
console.log("translationJobSession.test.ts: ok");
