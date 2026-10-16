// Only the Rust backend supplies this application-owned external runtime root.
// No environment, global npm module, CLI login, or HTTP option is a fallback.
import {readFile,realpath} from 'node:fs/promises';
import {join,relative,isAbsolute,sep} from 'node:path';
import {pathToFileURL} from 'node:url';
export const SDK_VERSION='1.0.53';
const RUNTIME_VERSION='1.1.71';
const codes=new Set(['QODER_SDK_MISSING','QODER_SDK_PATH_INVALID','QODER_SDK_VERSION_UNSUPPORTED','QODER_SDK_LOAD_FAILED','QODER_SDK_RUNTIME_MISSING']);
function failure(code){return Object.assign(Error(code),{code});}
async function inside(root,path){
 const actual=await realpath(path),part=relative(root,actual);
 if(isAbsolute(part)||part==='..'||part.startsWith('..'+sep))throw failure('QODER_SDK_PATH_INVALID');
 return actual;
}
export async function loadSdkModule(runtimeRoot){
 if(typeof runtimeRoot!=='string'||!isAbsolute(runtimeRoot))throw failure('QODER_SDK_PATH_INVALID');
 let root,packageRoot,metadata,entry;
 try{
  root=await realpath(runtimeRoot);
  packageRoot=await inside(root,join(root,'node_modules/@qoder-ai/qoder-agent-sdk'));
  metadata=JSON.parse(await readFile(await inside(root,join(packageRoot,'package.json')),'utf8'));
 }catch(error){throw codes.has(error.code)?error:failure(error.code==='ENOENT'?'QODER_SDK_MISSING':'QODER_SDK_LOAD_FAILED');}
 if(metadata.name!=='@qoder-ai/qoder-agent-sdk'||metadata.version!==SDK_VERSION)throw failure('QODER_SDK_VERSION_UNSUPPORTED');
 try{
  entry=await inside(packageRoot,join(packageRoot,'dist/index.js'));
  const sdk=await import(pathToFileURL(entry).href);
  if(typeof sdk.query!=='function'||typeof sdk.WorkerTransport!=='function')throw failure('QODER_SDK_LOAD_FAILED');
  return {sdk,packageRoot};
 }catch(error){throw codes.has(error.code)?error:failure('QODER_SDK_LOAD_FAILED');}
}
export async function loadExternalRuntime(runtimeRoot){
 const {sdk,packageRoot}=await loadSdkModule(runtimeRoot);
 try{
  const directory=await inside(packageRoot,join(packageRoot,'dist/_worker'));
  const info=JSON.parse(await readFile(await inside(packageRoot,join(directory,'runtime-info.json')),'utf8'));
  if(info.version!==RUNTIME_VERSION||info.profile!=='platform'||info.target!==`${process.platform}-${process.arch}`||info.build?.site!=='global'||info.build?.productName!=='qodercli')throw failure('QODER_SDK_VERSION_UNSUPPORTED');
  let workerPath;
  for(const name of ['qoder-worker-runtime.obf.mjs','qoder-worker-runtime.mjs']){
   try{workerPath=await inside(packageRoot,join(directory,name));break;}catch(error){if(codes.has(error.code))throw error;}
  }
  if(!workerPath)throw failure('QODER_SDK_RUNTIME_MISSING');
  if(process.platform!=='openharmony')await inside(packageRoot,join(directory,'vendor/ripgrep',`${process.arch}-${process.platform}`,process.platform==='win32'?'rg.exe':'rg'));
  const resolved=await inside(packageRoot,sdk.resolveQoderWorkerRuntimePath(workerPath));
  if(resolved!==workerPath)throw failure('QODER_SDK_PATH_INVALID');
  return {sdk,packageRoot,workerPath,runtimeVersion:RUNTIME_VERSION};
 }catch(error){throw codes.has(error.code)?error:failure('QODER_SDK_RUNTIME_MISSING');}
}
