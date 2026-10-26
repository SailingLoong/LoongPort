//! Native original-session generation owner. Selector bindings alone are never proof.
use super::{ProtectionRecord,ProtectionStorage};
use crate::{app_store::isolated_scope::{self,CandidateBinding,SelectionSnapshot,PublicationOutcome},config_file_io as io,database::{inspection,storage},error::AppError,secrets::{key_store::{self,KeyStore},session,storage::{StorageContext,PlaintextIdentity},VaultContext}};
use super::generation_files::{PreparedGenerationFiles,GenerationFileRole,GenerationSourceRoot};
use rusqlite::Connection;
use serde::{Serialize,Deserialize};
use sha2::{Digest,Sha256};
use std::{path::{Path,PathBuf,Component},sync::{MutexGuard,RwLockWriteGuard},io::Read};
use zeroize::Zeroizing;
fn invalid()->AppError{AppError::Config("protection.generation_changed".into())}
fn unfinished()->AppError{AppError::Config("protection.generation_not_ready".into())}
pub(crate) enum GenerationMode{Encrypted(Zeroizing<String>),Plaintext}
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum GenerationBoundary{Prepared,RuntimeRetired,SelectorPublished,CleanupFileRemoved}
type GenerationHook<'a> = &'a mut dyn FnMut(GenerationBoundary,&Path)->Result<(),AppError>;

#[derive(Clone,PartialEq,Eq)]
struct MetadataImage{record:Option<Vec<u8>>,vault:Option<Vec<u8>>}
impl MetadataImage{
 fn capture(root:&Path)->Result<Self,AppError>{Ok(Self{record:super::read_with_bytes(root)?.map(|(_,b)|b),vault:session::read_metadata_with_bytes(root)?.map(|(_,bytes)|bytes)})}
 fn verify(&self,root:&Path)->Result<(),AppError>{if *self!=Self::capture(root)?{return Err(invalid())}Ok(())}
}
/// The private constructor borrows the original session-write/connection guards.
pub(crate) struct GenerationPreparationPermit<'a>{db:&'a crate::Database,context:&'a StorageContext,conn:&'a Connection,selection:&'a SelectionSnapshot,device:&'a Path,metadata:&'a MetadataImage,token:&'a str}
impl GenerationPreparationPermit<'_>{
 pub(crate) fn verify_selection(&self,s:&SelectionSnapshot)->Result<(),AppError>{
  if s!=self.selection || s.source_data_root()!=self.db.secret_session().root() || s.source_device_root()!=self.device{return Err(invalid())}
  self.db.secret_session().ensure_available()?;self.metadata.verify(self.source_data_root())?;storage::check_identity(self.conn,self.context)
 }
 pub(crate) fn source_data_root(&self)->&Path{self.db.secret_session().root()}
 pub(crate) fn source_device_root(&self)->&Path{self.device}
 pub(crate) fn inspection_token(&self)->&str{self.token}
}
#[derive(Clone,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct BindingRecord{root_id:String,root:PathBuf,data:PathBuf,device:PathBuf,source_id:Option<String>,source_identity:Option<String>,selection:String,inspection:String}
impl BindingRecord{fn from(b:&CandidateBinding)->Self{Self{root_id:b.root_id().into(),root:b.root().into(),data:b.source_data_root().into(),device:b.source_device_root().into(),source_id:b.source_root_id().map(str::to_owned),source_identity:b.source_identity_digest().map(str::to_owned),selection:b.source_selection_digest().into(),inspection:b.inspection_token().into()}}}
#[derive(Clone,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct FileStamp{relative:PathBuf,hash:String,mode:u32}
#[derive(Clone,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct TreeStamp{files:Vec<FileStamp>,directories:Vec<PathBuf>}
#[derive(Clone,Copy,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase")]
enum SourceRoot{Data,Device}
#[derive(Clone,Copy,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase")]
enum CleanupKind{Owned,Device,Settings,Database,Sidecar}
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct CleanupFile{location:SourceRoot,stamp:FileStamp,kind:CleanupKind,target:PathBuf}
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct CleanupManifest{version:u32,binding:BindingRecord,targets:TreeStamp,files:Vec<CleanupFile>}
fn relative(path:&Path)->Result<(),AppError>{if path.as_os_str().is_empty()||path.components().any(|p|!matches!(p,Component::Normal(_))){return Err(invalid())}Ok(())}
fn stamp(root:&Path,path:&Path)->Result<FileStamp,AppError>{
 relative(path)?;let absolute=root.join(path);io::directory_exists_no_alias(absolute.parent().ok_or_else(invalid)?)?;
 let before=std::fs::symlink_metadata(&absolute).map_err(|e|AppError::io(&absolute,e))?;
 if !before.is_file()||before.file_type().is_symlink(){return Err(invalid())}
 #[cfg(unix)]{use std::os::unix::fs::MetadataExt;if before.nlink()!=1{return Err(invalid())}}
 let mut file=io::open_regular_file(&absolute,u64::MAX).map_err(|e|AppError::io(&absolute,e))?.ok_or_else(invalid)?;let opened=file.metadata().map_err(|e|AppError::io(&absolute,e))?;
 #[cfg(unix)]{use std::os::unix::fs::MetadataExt;if before.dev()!=opened.dev()||before.ino()!=opened.ino()||opened.nlink()!=1{return Err(invalid())}}
 let mut hash=Sha256::new();let mut buffer=Zeroizing::new([0u8;64*1024]);loop{let n=file.read(&mut *buffer).map_err(|e|AppError::io(&absolute,e))?;if n==0{break}hash.update(&buffer[..n]);}
 let after=std::fs::symlink_metadata(&absolute).map_err(|e|AppError::io(&absolute,e))?;
 if before.len()!=after.len()||before.modified().ok()!=after.modified().ok(){return Err(invalid())}
 #[cfg(unix)]let mode={use std::os::unix::fs::MetadataExt;if before.dev()!=after.dev()||before.ino()!=after.ino()||before.mode()!=after.mode()||after.nlink()!=1{return Err(invalid())}before.mode()&0o7777};
 #[cfg(not(unix))]let mode=0;
 Ok(FileStamp{relative:path.into(),hash:hex::encode(hash.finalize()),mode})
}
fn target_tree(root:&Path)->Result<TreeStamp,AppError>{
 fn walk(root:&Path,path:&Path,tree:&mut TreeStamp)->Result<(),AppError>{
  io::directory_exists_no_alias(path)?;
  for entry in std::fs::read_dir(path).map_err(|e|AppError::io(path,e))?{let entry=entry.map_err(|e|AppError::io(path,e))?;let p=entry.path();let rel=p.strip_prefix(root).map_err(|_|invalid())?;
   if path==root&&(rel==Path::new(super::FILE)||crate::panic_hook::diagnostic_entries().contains(&entry.file_name())){continue}
   let meta=std::fs::symlink_metadata(&p).map_err(|e|AppError::io(&p,e))?;if meta.file_type().is_symlink(){return Err(invalid())}
   if meta.is_dir(){tree.directories.push(rel.into());walk(root,&p,tree)?}else{tree.files.push(stamp(root,rel)?)}
  }Ok(())
 }
 let mut tree=TreeStamp{files:Vec::new(),directories:Vec::new()};walk(root,root,&mut tree)?;tree.files.sort_by(|a,b|a.relative.cmp(&b.relative));tree.directories.sort();Ok(tree)
}
fn write_record(root:&Path,record:&ProtectionRecord)->Result<(),AppError>{let bytes=serde_json::to_vec(record).map_err(|_|invalid())?;ProtectionRecord::parse(&bytes)?;io::write_durable(&root.join(super::FILE),&bytes)?;if super::read_with_bytes(root)?.map(|(_,b)|b)!=Some(bytes){return Err(invalid())}Ok(())}
fn write_image(root:&Path,relative:&Path,image:&Connection,context:&StorageContext)->Result<(),AppError>{
 relative_path_parent(root,relative)?;storage::check_identity(image,context)?;let bytes=image.serialize(rusqlite::MAIN_DB)?;
 io::write_durable(&root.join(relative),&bytes)?;
 let captured=inspection::capture(&root.join(relative))?.ok_or_else(invalid)?;storage::check_identity(&captured.image,context)?;
 if crate::Database::content_digest(image)?!=crate::Database::content_digest(&captured.image)?{return Err(invalid())}Ok(())
}
fn relative_path_parent(root:&Path,relative:&Path)->Result<(),AppError>{self::relative(relative)?;session::ensure_owned_directory(root,root.join(relative).parent().ok_or_else(invalid)?) }
fn database_cleanup(root:&Path,relative:&Path)->Result<Vec<CleanupFile>,AppError>{
 let mut files=Vec::new();for suffix in ["","-wal","-shm","-journal"]{
  let mut name=relative.as_os_str().to_os_string();name.push(suffix);let rel=PathBuf::from(name);let path=root.join(&rel);
  match std::fs::symlink_metadata(&path){Ok(_)=>files.push(CleanupFile{location:SourceRoot::Data,stamp:stamp(root,&rel)?,kind:if suffix.is_empty(){CleanupKind::Database}else{CleanupKind::Sidecar},target:relative.into()}),Err(e)if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(AppError::io(path,e))}
 }Ok(files)
}
/// Read the original durable selector, never a cached or caller-supplied root.
pub(crate) fn capture_current_selection()->Result<SelectionSnapshot,AppError>{
 let store=crate::app_store::tauri_store_path().ok_or_else(invalid)?;
 isolated_scope::capture_selection_at(&store,&crate::config::get_default_app_config_dir(),&crate::config::get_home_dir().join(crate::config::APP_DIR_NAME))
}
pub(crate) fn convert_at(db:&crate::Database,device:&Path,selection:SelectionSnapshot,mode:GenerationMode,store:&dyn KeyStore)->Result<PublicationOutcome,AppError>{convert_with_hook(db,device,selection,mode,store,&mut|_,_|Ok(()))}
fn pending_target(root:&Path,mode:GenerationMode)->Result<(StorageContext,Option<Zeroizing<String>>),AppError>{
 let record=super::read(root)?.ok_or_else(invalid)?;if !record.choice_complete{return Err(invalid())}
 let local=session::read_metadata_with_bytes(root)?.map(|(local,_)|local);
 let (target,text)=match mode{
  GenerationMode::Encrypted(text)=>{
   let local=local.as_ref().ok_or_else(invalid)?;
   let vault=VaultContext::from_user_text(local.metadata.clone(),&text).map_err(crate::secrets::error::secret_error)?;
   record.validate_vault(Some(vault.metadata()))?;(StorageContext::from(vault),Some(text))
  },
  GenerationMode::Plaintext=>{
   if local.is_some(){return Err(invalid())}
   (super::open_context(&record,None,None,|_,_|Err(invalid()))?,None)
  },
 };
 record.matches_context(&target)?;record.verify_commitment(&target)?;session::validate_completed_metadata(root,&target)?;Ok((target,text))
}
fn convert_with_hook<'a>(db:&'a crate::Database,device:&Path,selection:SelectionSnapshot,mode:GenerationMode,store:&dyn KeyStore,hook:GenerationHook<'a>)->Result<PublicationOutcome,AppError>{
 if matches!(&mode,GenerationMode::Encrypted(text) if text.is_empty()){return Err(AppError::Config("protection.key_input_required".into()))}
 let context=db.secret_session().write()?;let conn=db.conn.lock()?;storage::check_identity(&conn,&context)?;
 let root=db.secret_session().root();let metadata=MetadataImage::capture(root)?;
 if let Some((record,_))=super::read_with_bytes(root)?{record.matches_context(&context)?;record.verify_commitment(&context)?;if record.cleanup.is_some(){return Err(invalid())}}
 let primary=inspection::capture(&root.join(crate::config::DB_FILE_NAME))?.ok_or_else(invalid)?;
 inspection::verify_connection_primary_identity(&conn,&primary.revision.primary_identity()?)?;
 let logical=crate::Database::content_digest(&conn)?;if logical!=crate::Database::content_digest(&primary.image)?{return Err(invalid())}
 let token=hex::encode(Sha256::digest(serde_json::to_vec(&(selection.selection_digest(),context.identity().store_id(),context.identity().generation_id(),context.identity().revision())).map_err(|_|invalid())?));
 let permit=GenerationPreparationPermit{db,context:&context,conn:&conn,selection:&selection,device,metadata:&metadata,token:&token};permit.verify_selection(&selection)?;
 let (pending,target,text,resuming)=if let Some(binding)=isolated_scope::inspect_pending_generation_at(&selection)?{
  let (target,text)=pending_target(binding.root(),mode)?;let target_metadata=MetadataImage::capture(binding.root())?;
  // A plaintext source's earlier prepared target already has an authenticated
  // complete target image. Never refresh changed target bytes into that intent.
  if context.is_plaintext(){
   let record=super::read(binding.root())?.ok_or_else(invalid)?;
   let intent=record.cleanup_manifest(target.encrypted()?)?.ok_or_else(invalid)?;
   let manifest:CleanupManifest=serde_json::from_slice(&intent).map_err(|_|invalid())?;
   if manifest.version!=1||manifest.binding!=BindingRecord::from(&binding)||target_tree(binding.root())?!=manifest.targets{return Err(invalid())}
   for file in &manifest.files{allowed_cleanup(file)?;let source=match file.location{SourceRoot::Data=>root,SourceRoot::Device=>device};
    // SQLite may legitimately checkpoint/reopen its physical image after a
    // dead process. Its complete logical equality is checked below; original
    // permission semantics still may not be recaptured from changed input.
    if matches!(file.kind,CleanupKind::Database|CleanupKind::Sidecar){
     if source.join(&file.stamp.relative).exists()&&stamp(source,&file.stamp.relative)?.mode!=file.stamp.mode{return Err(invalid())}
    }else if stamp(source,&file.stamp.relative)?!=file.stamp{return Err(invalid())}
   }
  }
  let resume=GenerationResumePermit{source:permit,binding,target_metadata,target:target.clone()};
  let pending=isolated_scope::resume_generation_at(&selection,&resume)?;(pending,target,text,true)
 }else{
  let pending=isolated_scope::prepare_generation_at(&selection,&permit)?;
  let identity=context.identity();let revision=identity.revision().checked_add(1).ok_or_else(invalid)?;
  let (target,text)=match mode{
   GenerationMode::Encrypted(text)=>{let target=VaultContext::create_generation_with_user_text(identity.store_id().into(),revision,&text).map_err(crate::secrets::error::secret_error)?;(StorageContext::from(target),Some(text))},
   GenerationMode::Plaintext=>(StorageContext::Plaintext(PlaintextIdentity{format_version:1,store_id:identity.store_id().into(),generation_id:uuid::Uuid::new_v4().to_string(),revision}),None),
  };(pending,target,text,false)
 };
 let binding=pending.binding().clone();
 let prepared=if resuming{PreparedGenerationFiles::inspect_existing(root,device,pending.root(),&context,&target)?}else{PreparedGenerationFiles::stage(root,device,pending.root(),&context,&target)?};
 if resuming{
  let image=inspection::capture(&pending.root().join(crate::config::DB_FILE_NAME))?.ok_or_else(invalid)?;
  storage::verify_storage_generation_image(&conn,&context,&image.image,&target)?;
 }else{let image=storage::prepare_storage_generation(&conn,&context,&target)?;write_image(pending.root(),Path::new(crate::config::DB_FILE_NAME),&image,&target)?;}
 let mut backups=Vec::new();let mut cleanup_files=Vec::new();
 for evidence in prepared.evidence(){
  let kind=match evidence.role(){GenerationFileRole::Owned=>CleanupKind::Owned,GenerationFileRole::Device=>CleanupKind::Device,GenerationFileRole::Settings=>CleanupKind::Settings,GenerationFileRole::Neutral=>continue};
  let (location,source_root)=match evidence.source_root(){GenerationSourceRoot::Data=>(SourceRoot::Data,root),GenerationSourceRoot::Device=>(SourceRoot::Device,device)};
  let original=stamp(source_root,evidence.relative_path())?;if original.hash!=hex::encode(evidence.source_hash()){return Err(invalid())}
  cleanup_files.push(CleanupFile{location,stamp:original,kind,target:evidence.relative_path().into()});
 }
 let source_backups=crate::secrets::transition::database_backups(root)?;
 for path in &source_backups{
  let captured=inspection::capture(path)?.ok_or_else(invalid)?;let relative=path.strip_prefix(root).map_err(|_|invalid())?;
  if resuming{let image=inspection::capture(&pending.root().join(relative))?.ok_or_else(invalid)?;storage::verify_storage_backup_generation_image(&captured.image,&context,&image.image,&target)?;}
  else{let next=storage::prepare_storage_backup_generation_from_image(&captured.image,&context,&target)?;write_image(pending.root(),relative,&next,&target)?;}
  cleanup_files.extend(database_cleanup(root,relative)?);backups.push((path.clone(),captured.revision));
 }
 let relative_backups=|base:&Path,paths:Vec<PathBuf>|->Result<Vec<PathBuf>,AppError>{let mut paths=paths.into_iter().map(|path|path.strip_prefix(base).map(Path::to_path_buf).map_err(|_|invalid())).collect::<Result<Vec<_>,_>>()?;paths.sort();Ok(paths)};
 if relative_backups(root,source_backups)?!=relative_backups(pending.root(),crate::secrets::transition::database_backups(pending.root())?)?{return Err(invalid())}
 cleanup_files.extend(database_cleanup(root,Path::new(crate::config::DB_FILE_NAME))?);
 if !resuming{if let StorageContext::Encrypted(vault)=&target{session::write_metadata(pending.root(),&session::completed_metadata(vault,true)?)?;}}
 let manifest=CleanupManifest{version:1,binding:BindingRecord::from(&binding),targets:target_tree(pending.root())?,files:cleanup_files};
 let record=match &target{StorageContext::Encrypted(vault)=>ProtectionRecord::encrypted(vault,if context.is_plaintext(){Some(serde_json::to_string(&manifest).map_err(|_|invalid())?)}else{None}.as_deref())?,StorageContext::Plaintext(identity)=>ProtectionRecord{format_version:1,choice_complete:true,storage:ProtectionStorage::Plaintext{identity:identity.clone()},commitment:None,cleanup:None}};
 write_record(pending.root(),&record)?;
 if let (StorageContext::Encrypted(vault),Some(text))=(&target,text){key_store::save_user_text_verified(store,vault,&text).map_err(|_|AppError::Config("protection.key_cache_unavailable".into()))?;}
 hook(GenerationBoundary::Prepared,binding.root())?;prepared.verify_source()?;prepared.verify_target()?;
 let proof=VerifiedGeneration{db,context,conn,binding,metadata,primary:primary.revision,backups,prepared,target,manifest,logical,retired:false,hook};
 Ok(pending.publish(proof))
}
/// Only convert_at moves both original guards plus complete readback evidence here.
pub(crate) struct VerifiedGeneration<'a>{db:&'a crate::Database,context:RwLockWriteGuard<'a,StorageContext>,conn:MutexGuard<'a,Connection>,binding:CandidateBinding,metadata:MetadataImage,primary:inspection::SourceRevision,backups:Vec<(PathBuf,inspection::SourceRevision)>,prepared:PreparedGenerationFiles,target:StorageContext,manifest:CleanupManifest,logical:String,retired:bool,hook:GenerationHook<'a>}
impl VerifiedGeneration<'_>{
 pub(crate) fn binding(&self)->&CandidateBinding{&self.binding}
 fn verify_target(&self)->Result<(),AppError>{self.prepared.verify_target()?;if target_tree(self.binding.root())?!=self.manifest.targets{return Err(invalid())}let record=super::read(self.binding.root())?.ok_or_else(invalid)?;record.matches_context(&self.target)?;record.verify_commitment(&self.target)}
 pub(crate) fn begin_publication(&mut self,binding:&CandidateBinding)->Result<(),AppError>{
  if binding!=&self.binding||self.retired{return Err(invalid())}
  self.db.secret_session().ensure_available()?;self.metadata.verify(binding.source_data_root())?;self.prepared.verify_source()?;self.verify_target()?;
  storage::check_identity(&self.conn,&self.context)?;inspection::verify_unchanged(&binding.source_data_root().join(crate::config::DB_FILE_NAME),&self.primary)?;
  for(path,revision)in &self.backups{inspection::verify_unchanged(path,revision)?;}
  self.db.secret_session().set_blocked(true);self.retired=true;
  if self.context.is_plaintext(){
   // Close the original SQLite handle before deleting any primary/WAL/SHM.
   // A tombstone connection stays behind the permanently retired session fence.
   let old=std::mem::replace(&mut *self.conn,Connection::open_in_memory()?);old.close().map_err(|(_,e)|AppError::Database(e.to_string()))?;
   let source=inspection::capture(&binding.source_data_root().join(crate::config::DB_FILE_NAME))?.ok_or_else(invalid)?;
   if crate::Database::content_digest(&source.image)?!=self.logical{return Err(invalid())}
   self.manifest.files.retain(|f|!(f.location==SourceRoot::Data&&matches!(f.kind,CleanupKind::Database|CleanupKind::Sidecar)&&f.target==Path::new(crate::config::DB_FILE_NAME)));
   self.manifest.files.extend(database_cleanup(binding.source_data_root(),Path::new(crate::config::DB_FILE_NAME))?);
   let vault=self.target.encrypted()?;let intent=serde_json::to_string(&self.manifest).map_err(|_|invalid())?;write_record(binding.root(),&ProtectionRecord::encrypted(vault,Some(&intent))?)?;
  }
  (self.hook)(GenerationBoundary::RuntimeRetired,binding.root())?;
  Ok(())
 }
 pub(crate) fn complete_publication(&mut self,binding:&CandidateBinding)->Result<(),AppError>{
  if !self.retired||binding!=&self.binding{return Err(invalid())}(self.hook)(GenerationBoundary::SelectorPublished,binding.root())?;self.verify_target()?;
  if self.context.is_plaintext(){finish_cleanup(&self.manifest,self.target.encrypted()?,self.hook)?;}Ok(())
 }
}
fn allowed_cleanup(entry:&CleanupFile)->Result<(),AppError>{
 relative(&entry.stamp.relative)?;relative(&entry.target)?;
 let rel=&entry.stamp.relative;let stripped=rel.strip_prefix("backups/vault-recovery").unwrap_or(rel);
 match entry.kind{
  CleanupKind::Owned=>{crate::secrets::files::OwnedFile::registered(stripped)?;},
  CleanupKind::Device=>{crate::secrets::owned_file::DeviceFile::registered(stripped)?;},
  CleanupKind::Settings=>{if stripped!=Path::new("settings.json"){return Err(invalid())}},
  CleanupKind::Database|CleanupKind::Sidecar=>{if entry.location!=SourceRoot::Data{return Err(invalid())}let base=&entry.target;if base!=Path::new(crate::config::DB_FILE_NAME)&&!(base.parent()==Some(Path::new("backups"))&&base.extension().is_some_and(|s|s=="db")){return Err(invalid())}let allowed=["","-wal","-shm","-journal"].iter().any(|suffix|{let mut p=base.as_os_str().to_os_string();p.push(suffix);Path::new(&p)==rel});if !allowed{return Err(invalid())}},
 }Ok(())
}
fn finish_cleanup(manifest:&CleanupManifest,vault:&VaultContext,hook:GenerationHook<'_>)->Result<(),AppError>{
 if manifest.version!=1||manifest.binding.root.starts_with(&manifest.binding.data)||manifest.binding.data.starts_with(&manifest.binding.root)||manifest.binding.root.starts_with(&manifest.binding.device)||manifest.binding.device.starts_with(&manifest.binding.root){return Err(invalid())}
 if target_tree(&manifest.binding.root)?!=manifest.targets{return Err(invalid())}
 // Preflight every remaining source before deleting even one owned file.
 for entry in &manifest.files{allowed_cleanup(entry)?;let root=match entry.location{SourceRoot::Data=>&manifest.binding.data,SourceRoot::Device=>&manifest.binding.device};let path=root.join(&entry.stamp.relative);
  match std::fs::symlink_metadata(&path){Ok(_)=>if stamp(root,&entry.stamp.relative)?!=entry.stamp{return Err(invalid())},Err(e)if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(AppError::io(path,e))}
 }
 for entry in &manifest.files{let root=match entry.location{SourceRoot::Data=>&manifest.binding.data,SourceRoot::Device=>&manifest.binding.device};let path=root.join(&entry.stamp.relative);
  match std::fs::symlink_metadata(&path){Ok(_)=>{if stamp(root,&entry.stamp.relative)?!=entry.stamp{return Err(invalid())}std::fs::remove_file(&path).map_err(|e|AppError::io(&path,e))?;sync_parent(&path)?;hook(GenerationBoundary::CleanupFileRemoved,&path)?;},Err(e)if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(AppError::io(path,e))}
 }
 if target_tree(&manifest.binding.root)?!=manifest.targets{return Err(invalid())}
 write_record(&manifest.binding.root,&ProtectionRecord::encrypted(vault,None)?)
}
fn sync_parent(path:&Path)->Result<(),AppError>{#[cfg(unix)]{let parent=path.parent().ok_or_else(invalid)?;std::fs::File::open(parent).and_then(|f|f.sync_all()).map_err(|e|AppError::io(parent,e))?;}Ok(())}

pub(crate) struct SeparateConfigurationPermit<'a>{_owner:&'a crate::secrets::startup::StartupCoordinator}
impl SeparateConfigurationPermit<'_>{
 pub(crate) fn verify_selection(&self,_:&SelectionSnapshot)->Result<(),AppError>{Err(unfinished())}
 pub(crate) fn source_data_root(&self)->&Path{Path::new("")}
 pub(crate) fn source_device_root(&self)->&Path{Path::new("")}
 pub(crate) fn inspection_token(&self)->&str{""}
}
pub(crate) struct VerifiedSeparateConfiguration<'a>{binding:CandidateBinding,_owner:&'a crate::secrets::startup::StartupCoordinator}
impl VerifiedSeparateConfiguration<'_>{
 pub(crate) fn binding(&self)->&CandidateBinding{&self.binding}
 pub(crate) fn begin_publication(&mut self,_:&CandidateBinding)->Result<(),AppError>{Err(unfinished())}
 pub(crate) fn complete_publication(&mut self,_:&CandidateBinding)->Result<(),AppError>{Err(unfinished())}
}
pub(crate) struct GenerationResumePermit<'a>{source:GenerationPreparationPermit<'a>,binding:CandidateBinding,target_metadata:MetadataImage,target:StorageContext}
impl GenerationResumePermit<'_>{
 pub(crate) fn binding(&self)->&CandidateBinding{&self.binding}
 pub(crate) fn verify_selection(&self,selection:&SelectionSnapshot)->Result<(),AppError>{
  self.source.verify_selection(selection)?;
  if self.binding.inspection_token()!=self.source.inspection_token(){return Err(invalid())}
  self.target_metadata.verify(self.binding.root())?;
  let record=super::read(self.binding.root())?.ok_or_else(invalid)?;record.matches_context(&self.target)?;record.verify_commitment(&self.target)
 }
}
/// Constructed only after the selected record, target and cleanup intent authenticate.
pub(crate) struct GenerationRecoveryPermit<'a>{selection:&'a SelectionSnapshot,binding:&'a CandidateBinding,metadata:&'a MetadataImage,context:&'a StorageContext,manifest:&'a CleanupManifest}
impl GenerationRecoveryPermit<'_>{
 pub(crate) fn verify_selection(&self,selection:&SelectionSnapshot)->Result<(),AppError>{
  if selection!=self.selection||selection.selected_generation_binding().as_ref()!=Some(self.binding)||self.manifest.version!=1||self.manifest.binding!=BindingRecord::from(self.binding){return Err(invalid())}
  self.metadata.verify(self.binding.root())?;
  let record=super::read(self.binding.root())?.ok_or_else(invalid)?;record.matches_context(self.context)?;record.verify_commitment(self.context)?;
  if record.cleanup.is_none()||target_tree(self.binding.root())?!=self.manifest.targets{return Err(invalid())}Ok(())
 }
 pub(crate) fn selected_root_id(&self)->&str{self.binding.root_id()}
}
#[cfg(any(test,feature="test-hooks"))]
#[path="generation_tests.rs"]
pub(crate) mod tests;

/// Private authenticated selected-cleanup receipt; no caller can manufacture completion.
pub(crate) struct CompletedCleanup {root:PathBuf,before:Vec<u8>,after:Vec<u8>}
impl CompletedCleanup {
 pub(crate) fn root(&self)->&Path{&self.root}
 pub(crate) fn before(&self)->&[u8]{&self.before}
 pub(crate) fn after(&self)->&[u8]{&self.after}
}
pub(crate) fn recover_selected_at(root:&Path,selection:&SelectionSnapshot,store:&dyn KeyStore,provided:Option<Zeroizing<Vec<u8>>>)->Result<CompletedCleanup,AppError>{
 let binding=selection.selected_generation_binding().ok_or_else(invalid)?;
 if binding.root()!=root||selection.source_data_root()!=root||selection.source_device_root()!=root{return Err(invalid())}
 let metadata=MetadataImage::capture(root)?;
 let (record,before)=super::read_with_bytes(root)?.ok_or_else(invalid)?;
 let local=session::read_metadata(root)?;
 let context=super::open_cleanup_context(&record,Some(&local.metadata),provided,|vault,key|store.load(vault,key).map_err(|_|AppError::Config("secret.store_unavailable".into())))?;
 let vault=context.encrypted()?;
 let bytes=record.cleanup_manifest(vault)?.ok_or_else(invalid)?;
 let manifest:CleanupManifest=serde_json::from_slice(&bytes).map_err(|_|invalid())?;
 let permit=GenerationRecoveryPermit{selection,binding:&binding,metadata:&metadata,context:&context,manifest:&manifest};
 permit.verify_selection(selection)?;
 let _lock=isolated_scope::recover_selection_lock_at(selection,&permit)?;
 finish_cleanup(&manifest,vault,&mut|_,_|Ok(()))?;
 let (complete,after)=super::read_with_bytes(root)?.ok_or_else(invalid)?;
 complete.matches_context(&context)?;complete.verify_commitment(&context)?;
 if complete.cleanup.is_some()||target_tree(root)?!=manifest.targets{return Err(invalid())}
 Ok(CompletedCleanup{root:root.into(),before,after})
}
