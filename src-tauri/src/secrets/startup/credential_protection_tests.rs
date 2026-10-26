use super::*;
use crate::secrets::{key_store::KeyStoreError,protection::{ProtectionRecord},session,testing::TestHome,VaultContext};
use serde_json::{json,Value};
use std::sync::{Mutex,atomic::{AtomicUsize,Ordering}};
use tauri::{Manager,Emitter};
use tauri::test::MockRuntime;
struct Cache { bytes:Mutex<Option<Vec<u8>>>,denied:bool,loads:AtomicUsize,writes:AtomicUsize }
impl Cache { fn new(denied:bool)->Self {Self{bytes:Mutex::new(None),denied,loads:AtomicUsize::new(0),writes:AtomicUsize::new(0)}} }
impl KeyStore for Cache {
    fn load(&self,_:&str,_:&str)->Result<Option<Zeroizing<Vec<u8>>>,KeyStoreError>{self.loads.fetch_add(1,Ordering::SeqCst);if self.denied{return Err(KeyStoreError::Unavailable)} Ok(self.bytes.lock().unwrap().clone().map(Zeroizing::new))}
    fn save(&self,_:&str,_:&str,bytes:&[u8])->Result<(),KeyStoreError>{self.writes.fetch_add(1,Ordering::SeqCst);if self.denied{return Err(KeyStoreError::Unavailable)}*self.bytes.lock().unwrap()=Some(bytes.to_vec());Ok(())}
    fn remove(&self,_:&str,_:&str)->Result<(),KeyStoreError>{panic!("never removes original recovery material")}
}
fn prepare(app:&AppHandle<MockRuntime>,session:Arc<session::SecretSession>)->Result<(),String>{
    let (state,_,_)=crate::initialize_runtime_core(session).map_err(|e|e.to_string())?;
    assert!(app.manage(state));Ok(())
}
fn initialize(coordinator:&StartupCoordinator,app:&AppHandle<MockRuntime>,key:Zeroizing<Vec<u8>>)->Result<(),String>{
    let dependencies=app.state::<CommandDependencies<MockRuntime>>();
    coordinator.initialize_with_internal_key(None,Some(key),&*dependencies.store,|session|prepare(app,session))?;
    crate::init_status::clear_init_error();app.emit("runtime-ready",()).map_err(|_|"secret.event_unavailable".to_owned())?;Ok(())
}
fn app(coordinator:StartupCoordinator,cache:Arc<Cache>)->tauri::App<MockRuntime>{
    tauri::test::mock_builder().manage(coordinator).manage(CommandDependencies{store:cache as Arc<dyn KeyStore>,initialize})
        .invoke_handler(tauri::generate_handler![get_credential_protection_status,unlock_credential_runtime,configure_credential_protection])
        .build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap()
}
fn invoke(window:&tauri::WebviewWindow<MockRuntime>,cmd:&str,body:Value)->Result<Value,Value>{
    tauri::test::get_ipc_response(window,tauri::webview::InvokeRequest{cmd:cmd.into(),callback:tauri::ipc::CallbackFn(0),error:tauri::ipc::CallbackFn(1),url:"tauri://localhost".parse().unwrap(),body:tauri::ipc::InvokeBody::Json(body),headers:Default::default(),invoke_key:tauri::test::INVOKE_KEY.into()})
        .map(|response|response.deserialize::<Value>().unwrap())
}
fn window(app:&tauri::App<MockRuntime>)->tauri::WebviewWindow<MockRuntime>{tauri::WebviewWindowBuilder::new(app,"main",Default::default()).build().unwrap()}
fn encrypted()->(std::path::PathBuf,VaultContext){
    let root=crate::config::get_app_config_dir();crate::config::ensure_private_directory(&root).unwrap();
    let vault=VaultContext::create_generation_with_user_text(uuid::Uuid::new_v4().to_string(),1," 中文🔑 aBc\r\n ").unwrap();
    session::write_metadata(&root,&session::completed_metadata(&vault,true).unwrap()).unwrap();
    let record=ProtectionRecord::encrypted(&vault,None).unwrap();
    crate::config_file_io::write_durable(&root.join("protection.json"),&serde_json::to_vec(&record).unwrap()).unwrap();(root,vault)
}
fn coordinator(root:&std::path::Path)->StartupCoordinator{StartupCoordinator::new(root.into(),crate::secrets::upgrade::inspect(root,&crate::live::engine::DeviceStore::for_device()).unwrap())}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_locked_status_needs_no_app_state_or_system_cache_read(){
    let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let(root,_)=encrypted();let cache=Arc::new(Cache::new(true));
    let app=app(coordinator(&root),cache.clone());let window=window(&app);
    assert!(app.try_state::<crate::AppState>().is_none());
    let result=invoke(&window,"get_credential_protection_status",json!({})).unwrap();
    assert_eq!(result["mode"],"encrypted");assert_eq!(result["status"],"key_required");assert_eq!(result["canUnlock"],true);
    assert!(result["reviewToken"].is_string());assert_eq!(cache.loads.load(Ordering::SeqCst),0);
}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_exact_text_recovers_existing_data_even_when_cache_is_denied(){
    let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let(root,vault)=encrypted();let cache=Arc::new(Cache::new(true));
    let co=coordinator(&root);let error=co.initialize_with(None,&*cache,|_|panic!("cache denied must not publish runtime")).unwrap_err();
    crate::init_status::set_init_error(super::super::startup_failure_payload(&error,false));
    let app=app(co,cache.clone());let window=window(&app);let before=std::fs::read(root.join("vault.json")).unwrap();
    let status=invoke(&window,"get_credential_protection_status",json!({})).unwrap();let token=status["reviewToken"].as_str().unwrap();
    assert!(invoke(&window,"unlock_credential_runtime",json!({"keyText":"wrong","reviewToken":token})).is_err());
    assert!(app.try_state::<crate::AppState>().is_none());assert_eq!(std::fs::read(root.join("vault.json")).unwrap(),before);assert_eq!(cache.writes.load(Ordering::SeqCst),0);
    let result=invoke(&window,"unlock_credential_runtime",json!({"keyText":" 中文🔑 aBc\r\n ","reviewToken":token})).unwrap();
    assert_eq!(result["status"],"available");assert_eq!(result["errorCode"],"protection.key_cache_unavailable");
    let state=app.state::<crate::AppState>();assert_eq!(state.db.secret_session().read().unwrap().encrypted().unwrap().metadata(),vault.metadata());
    assert!(app.state::<StartupCoordinator>().runtime_availability(Some(state.db.secret_session())).credential_runtime_available);
    assert!(!serde_json::to_string(&result).unwrap().contains("中文"));assert_eq!(std::fs::read(root.join("vault.json")).unwrap(),before);
}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_fresh_login_then_explicit_skip_persists_once_without_a_generation(){
    let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let root=crate::config::get_app_config_dir();let cache=Arc::new(Cache::new(false));
    let app=app(coordinator(&root),cache.clone());
    app.state::<StartupCoordinator>().initialize_with(None,&*cache,|session|prepare(app.handle(),session)).unwrap();let window=window(&app);
    let first=invoke(&window,"get_credential_protection_status",json!({})).unwrap();assert_eq!(first["status"],"available");assert_eq!(first["shouldPrompt"],false);
    let before=app.state::<crate::AppState>().db.secret_session().read().unwrap().identity();
    let mut settings=crate::settings::get_settings_for_frontend().unwrap();settings.service_onboarding_completed=true;crate::settings::update_settings(settings).unwrap();
    let prompted=invoke(&window,"get_credential_protection_status",json!({})).unwrap();assert_eq!(prompted["shouldPrompt"],true);assert_eq!(prompted["canSwitchToPlaintext"],true);
    let body=json!({"request":{"mode":"plaintext","preserveExisting":false,"plaintextAcknowledged":true,"reviewToken":prompted["reviewToken"]}});
    let selected=invoke(&window,"configure_credential_protection",body.clone()).unwrap();assert_eq!(selected["shouldPrompt"],false);assert_eq!(selected["restartRequired"],false);
    let repeated=invoke(&window,"configure_credential_protection",body).unwrap();assert_eq!(repeated["status"],"available");
    assert_eq!(app.state::<crate::AppState>().db.secret_session().read().unwrap().identity(),before);
    assert!(crate::secrets::protection::read(&root).unwrap().unwrap().choice_complete);assert!(!root.join("vault.json").exists());assert_eq!(cache.writes.load(Ordering::SeqCst),0);
}
#[cfg(feature="test-hooks")]
pub(crate) fn run_case(case:&str){match case{"locked"=>real_ipc_locked_status_needs_no_app_state_or_system_cache_read(),"manual_cache_denied"=>real_ipc_exact_text_recovers_existing_data_even_when_cache_is_denied(),"fresh_skip"=>real_ipc_fresh_login_then_explicit_skip_persists_once_without_a_generation(),"stale_review"=>real_ipc_changed_metadata_rejects_old_review_before_key_cache(),"explicit_skip"=>real_ipc_plaintext_requires_explicit_action_and_survives_restart(),"legacy_prompt"=>real_ipc_legacy_upgrade_prompt_does_not_depend_on_new_onboarding_flag(),"plain_to_text"=>real_ipc_plain_to_user_text_restarts_with_same_user_text(),"text_to_plain"=>real_ipc_user_text_to_plaintext_requires_explicit_choice(),"rekey"=>real_ipc_user_text_rekey_preserves_authenticated_old_data(),"new_key_cache_failure"=>real_ipc_new_key_cache_failure_keeps_runtime_and_selection(),_=>panic!("unknown credential protection IPC case")}}

#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_changed_metadata_rejects_old_review_before_key_cache(){
    let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let(root,_)=encrypted();let cache=Arc::new(Cache::new(true));
    let application=app(coordinator(&root),cache.clone());let webview=window(&application);
    let status=invoke(&webview,"get_credential_protection_status",json!({})).unwrap();
    let mut metadata=session::read_metadata(&root).unwrap();metadata.automatic_unlock=false;session::write_metadata(&root,&metadata).unwrap();
    let before=std::fs::read(root.join("vault.json")).unwrap();
    assert!(invoke(&webview,"unlock_credential_runtime",json!({"keyText":" 中文🔑 aBc\r\n ","reviewToken":status["reviewToken"]})).is_err());
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(),before);assert_eq!(cache.loads.load(Ordering::SeqCst),0);
    assert_eq!(cache.writes.load(Ordering::SeqCst),0);assert!(application.try_state::<crate::AppState>().is_none());
}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_plaintext_requires_explicit_action_and_survives_restart(){
    if std::env::var_os("LOONGPORT_TEST_PROTECTION_RESTART_CHILD").is_some(){
        assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
        let root=crate::config::get_app_config_dir();let cache=Arc::new(Cache::new(false));
        let application=app(coordinator(&root),cache.clone());
        application.state::<StartupCoordinator>().initialize_with(None,&*cache,|session|prepare(application.handle(),session)).unwrap();
        let status=invoke(&window(&application),"get_credential_protection_status",json!({})).unwrap();
        assert_eq!(status["status"],"available");assert_eq!(status["shouldPrompt"],false);
        assert_eq!(cache.loads.load(Ordering::SeqCst),0);assert_eq!(cache.writes.load(Ordering::SeqCst),0);return;
    }

    let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let root=crate::config::get_app_config_dir();let cache=Arc::new(Cache::new(false));
    {
        let application=app(coordinator(&root),cache.clone());
        application.state::<StartupCoordinator>().initialize_with(None,&*cache,|session|prepare(application.handle(),session)).unwrap();let webview=window(&application);
        let mut settings=crate::settings::get_settings_for_frontend().unwrap();settings.service_onboarding_completed=true;crate::settings::update_settings(settings).unwrap();
        let status=invoke(&webview,"get_credential_protection_status",json!({})).unwrap();let before=std::fs::read(root.join("protection.json")).unwrap();
        for request in [json!({"mode":"plaintext","preserveExisting":false,"plaintextAcknowledged":false,"reviewToken":status["reviewToken"]}),
            json!({"mode":"plaintext","keyText":" ","preserveExisting":false,"plaintextAcknowledged":true,"reviewToken":status["reviewToken"]}),
            json!({"mode":"encrypted","keyText":"","preserveExisting":false,"plaintextAcknowledged":false,"reviewToken":status["reviewToken"]})] {
            assert!(invoke(&webview,"configure_credential_protection",json!({"request":request})).is_err());
            assert_eq!(std::fs::read(root.join("protection.json")).unwrap(),before);
        }
        let result=invoke(&webview,"configure_credential_protection",json!({"request":{"mode":"plaintext","preserveExisting":false,"plaintextAcknowledged":true,"reviewToken":status["reviewToken"]}})).unwrap();
        assert_eq!(result["shouldPrompt"],false);
    }
    // A real fresh process, not a test-only reset of original static lifecycle owners.
    let test_name=if cfg!(test){"secrets::startup::credential_protection::tests::real_ipc_plaintext_requires_explicit_action_and_survives_restart"}else{"protection_ipc_explicit_skip"};
    let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact",test_name,"--test-threads=1","--nocapture"])
        .env("LOONGPORT_TEST_PROTECTION_RESTART_CHILD","1").output().unwrap();
    assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_legacy_upgrade_prompt_does_not_depend_on_new_onboarding_flag(){
    let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let root=crate::config::get_app_config_dir();crate::config::ensure_private_directory(&root).unwrap();
    let legacy=VaultContext::generate().unwrap();session::write_metadata(&root,&session::completed_metadata(&legacy,true).unwrap()).unwrap();
    let cache=Arc::new(Cache::new(false));*cache.bytes.lock().unwrap()=Some(legacy.export_key().to_vec());
    let application=app(coordinator(&root),cache.clone());let webview=window(&application);
    let locked=invoke(&webview,"get_credential_protection_status",json!({})).unwrap();assert_eq!(locked["canUnlock"],false);
    application.state::<StartupCoordinator>().initialize_with(None,&*cache,|session|prepare(application.handle(),session)).unwrap();
    assert!(!crate::settings::get_settings_for_frontend().unwrap().service_onboarding_completed);
    let status=invoke(&webview,"get_credential_protection_status",json!({})).unwrap();
    assert_eq!(status["status"],"available");assert_eq!(status["shouldPrompt"],true);
    assert!(!root.join("protection.json").exists());assert_eq!(cache.writes.load(Ordering::SeqCst),0);
}

const CONVERSION_TEXT:&str=" 用户 原样🔐\r\n aBc ";
const CONVERSION_CANARY:&str="synthetic-ipc-generation-secret";
fn conversion_app(coordinator:StartupCoordinator,store:Arc<dyn KeyStore>)->tauri::App<MockRuntime>{
 tauri::test::mock_builder().manage(coordinator).manage(CommandDependencies{store,initialize})
 .invoke_handler(tauri::generate_handler![get_credential_protection_status,unlock_credential_runtime,configure_credential_protection])
 .build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap()
}
fn conversion_case(source_encrypted:bool,target_plaintext:bool,test_name:&str){
 if std::env::var_os("LOONGPORT_TEST_IPC_CONVERSION_CHILD").is_some(){
  assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());crate::app_store::initialize_active_scope().unwrap();
  let root=crate::config::get_app_config_dir();assert_eq!(crate::config::get_device_config_dir(),root);
  let cache=Arc::new(Cache::new(true));let application=app(coordinator(&root),cache.clone());let webview=window(&application);
  let result=if target_plaintext{
   application.state::<StartupCoordinator>().initialize_with(None,&*cache,|session|prepare(application.handle(),session)).unwrap();
   invoke(&webview,"get_credential_protection_status",json!({})).unwrap()
  }else{
   let status=invoke(&webview,"get_credential_protection_status",json!({})).unwrap();assert_eq!(status["canUnlock"],true);
   invoke(&webview,"unlock_credential_runtime",json!({"keyText":CONVERSION_TEXT,"reviewToken":status["reviewToken"]})).unwrap()
  };
  assert_eq!(result["status"],"available");assert_eq!(result["shouldPrompt"],false);assert_eq!(result["mode"],if target_plaintext{"plaintext"}else{"encrypted"});
  let state=application.state::<crate::AppState>();let context=state.db.secret_session().read().unwrap();let conn=state.db.conn.lock().unwrap();
  let raw:String=conn.query_row("SELECT settings_config FROM providers WHERE id='conversion'",[],|r|r.get(0)).unwrap();
  assert_eq!(crate::secrets::inventory::open_db(&context,"providers","settings_config",&["conversion","codex"],&raw).unwrap(),CONVERSION_CANARY);return;
 }
 let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let cache=Arc::new(crate::secrets::testing::MemoryKeyStore::default());
 let root=if source_encrypted{let(root,vault)=encrypted();crate::secrets::key_store::save_user_text_verified(&*cache,&vault," 中文🔑 aBc\r\n ").unwrap();root}else{crate::config::get_app_config_dir()};
 let application=conversion_app(coordinator(&root),cache.clone());
 application.state::<StartupCoordinator>().initialize_with(None,&*cache,|session|prepare(application.handle(),session)).unwrap();
 {
  let state=application.state::<crate::AppState>();let context=state.db.secret_session().read().unwrap();let conn=state.db.conn.lock().unwrap();
  let secret=crate::secrets::inventory::seal_db(&context,"providers","settings_config",&["conversion","codex"],CONVERSION_CANARY).unwrap();
  let meta=crate::secrets::inventory::seal_db(&context,"providers","meta",&["conversion","codex"],"{}").unwrap();
  conn.execute("INSERT INTO providers(id,app_type,name,settings_config,meta) VALUES('conversion','codex','Synthetic',?1,?2)",rusqlite::params![secret,meta]).unwrap();
 }
 let webview=window(&application);let status=invoke(&webview,"get_credential_protection_status",json!({})).unwrap();assert_eq!(status["canConfigure"],true);assert_eq!(status["canSwitchToPlaintext"],true);
 let mut request=json!({"mode":if target_plaintext{"plaintext"}else{"encrypted"},"preserveExisting":false,"plaintextAcknowledged":target_plaintext,"reviewToken":status["reviewToken"]});
 if !target_plaintext{request["keyText"]=json!(CONVERSION_TEXT);}
 let result=invoke(&webview,"configure_credential_protection",json!({"request":request.clone()})).unwrap();assert_eq!(result["restartRequired"],true);assert_ne!(result["status"],"available");
 assert!(application.state::<crate::AppState>().db.secret_session().read().is_err());
 let store=crate::app_store::tauri_store_path().unwrap();let selected=std::fs::read(&store).unwrap();
 let duplicate=invoke(&webview,"configure_credential_protection",json!({"request":request})).unwrap();assert_eq!(duplicate["restartRequired"],true);assert_eq!(std::fs::read(&store).unwrap(),selected);
 assert_eq!(std::fs::read_dir(store.parent().unwrap().join("isolated-configs")).unwrap().count(),1);
 let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact",test_name,"--test-threads=1","--nocapture"]).env("LOONGPORT_TEST_IPC_CONVERSION_CHILD","1").output().unwrap();
 assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_plain_to_user_text_restarts_with_same_user_text(){conversion_case(false,false,if cfg!(test){"secrets::startup::credential_protection::tests::real_ipc_plain_to_user_text_restarts_with_same_user_text"}else{"protection_ipc_plain_to_text"});}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_user_text_to_plaintext_requires_explicit_choice(){conversion_case(true,true,if cfg!(test){"secrets::startup::credential_protection::tests::real_ipc_user_text_to_plaintext_requires_explicit_choice"}else{"protection_ipc_text_to_plain"});}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_user_text_rekey_preserves_authenticated_old_data(){conversion_case(true,false,if cfg!(test){"secrets::startup::credential_protection::tests::real_ipc_user_text_rekey_preserves_authenticated_old_data"}else{"protection_ipc_rekey"});}
#[cfg_attr(test,test)]
#[cfg_attr(test,serial_test::serial)]
fn real_ipc_new_key_cache_failure_keeps_runtime_and_selection(){
 let _home=TestHome::new().unwrap();crate::init_status::clear_init_error();let root=crate::config::get_app_config_dir();let cache=Arc::new(Cache::new(true));let application=app(coordinator(&root),cache.clone());
 application.state::<StartupCoordinator>().initialize_with(None,&*cache,|session|prepare(application.handle(),session)).unwrap();let webview=window(&application);
 let status=invoke(&webview,"get_credential_protection_status",json!({})).unwrap();assert_eq!(status["canConfigure"],true);
 let error=invoke(&webview,"configure_credential_protection",json!({"request":{"mode":"encrypted","keyText":CONVERSION_TEXT,"preserveExisting":false,"plaintextAcknowledged":false,"reviewToken":status["reviewToken"]}})).unwrap_err();
 assert_eq!(error,"protection.key_cache_unavailable");assert!(application.state::<crate::AppState>().db.secret_session().read().is_ok());assert!(!crate::app_store::tauri_store_path().unwrap().exists());
}
