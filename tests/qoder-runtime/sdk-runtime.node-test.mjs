import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,symlink,cp,rm,realpath} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
const missing=async()=>{throw Error('RUNTIME_NOT_IMPLEMENTED');};
const {loadSdkModule=missing,loadExternalRuntime=missing}=await import('../../scripts/qoder/sdk-runtime.mjs').catch(()=>({}));
async function directory(t){const root=await mkdtemp(join(tmpdir(),'qoder-runtime-'));t.after(()=>rm(root,{recursive:true,force:true}));return root;}
test('missing fixed SDK package is reported without falling back to ambient modules',async t=>{
 const root=await directory(t);await assert.rejects(loadSdkModule(root),{code:'QODER_SDK_MISSING'});
});
test('SDK version is rejected before module evaluation',async t=>{
 const root=await directory(t),pkg=join(root,'node_modules/@qoder-ai/qoder-agent-sdk');await mkdir(pkg,{recursive:true});await writeFile(join(pkg,'package.json'),JSON.stringify({name:'@qoder-ai/qoder-agent-sdk',version:'0.0.1'}));
 await assert.rejects(loadSdkModule(root),{code:'QODER_SDK_VERSION_UNSUPPORTED'});
});
test('corrupt SDK metadata is a load failure rather than a missing installation',async t=>{
 const root=await directory(t),pkg=join(root,'node_modules/@qoder-ai/qoder-agent-sdk');await mkdir(pkg,{recursive:true});await writeFile(join(pkg,'package.json'),'{invalid');
 await assert.rejects(loadSdkModule(root),{code:'QODER_SDK_LOAD_FAILED'});
});
test('SDK package symlink cannot escape the application runtime root',async t=>{
 const root=await directory(t),outside=await directory(t);await mkdir(join(root,'node_modules/@qoder-ai'),{recursive:true});await symlink(outside,join(root,'node_modules/@qoder-ai/qoder-agent-sdk'),'junction');
 await assert.rejects(loadSdkModule(root),{code:'QODER_SDK_PATH_INVALID'});
});
const sdkRoot=process.env.LOONGPORT_QODER_SDK_TEST_ROOT;
test('official SDK1.0.53 module loads without starting a Query or using an account',{skip:!sdkRoot&&'official external SDK test root not supplied'},async()=>{
 const {sdk,packageRoot}=await loadSdkModule(sdkRoot);assert.equal(typeof sdk.query,'function');assert.equal(typeof sdk.WorkerTransport,'function');assert.ok(packageRoot.startsWith(await realpath(sdkRoot)));
});
test('official SDK remains loadable after relocating its complete external runtime directory',{skip:!sdkRoot&&'official external SDK test root not supplied'},async t=>{
 const dest=join(await directory(t),'relocated');await cp(sdkRoot,dest,{recursive:true,verbatimSymlinks:true});const {sdk,packageRoot}=await loadSdkModule(dest);assert.equal(typeof sdk.query,'function');assert.ok(packageRoot.startsWith(await realpath(dest)));
});
test('official Worker runtime is selected explicitly inside the pinned SDK package',{skip:!sdkRoot&&'official external SDK test root not supplied'},async()=>{
 const runtime=await loadExternalRuntime(sdkRoot);assert.ok(runtime.workerPath.startsWith(runtime.packageRoot));assert.equal(runtime.runtimeVersion,'1.1.71');
});
