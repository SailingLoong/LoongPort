#![cfg(feature="test-hooks")]
#[test]
#[serial_test::serial]
fn native_generation_plain_to_text(){cc_switch_lib::integration_test_support::verify_native_generation_case("plain_to_text");}
#[test]
#[serial_test::serial]
fn native_generation_text_to_plain(){cc_switch_lib::integration_test_support::verify_native_generation_case("text_to_plain");}
#[test]
#[serial_test::serial]
fn native_generation_cache_failure(){cc_switch_lib::integration_test_support::verify_native_generation_case("cache_failure");}

#[test]
#[serial_test::serial]
fn native_generation_large_record(){cc_switch_lib::integration_test_support::verify_native_generation_case("large_record");}

#[test]
#[serial_test::serial]
fn native_generation_before_selector(){cc_switch_lib::integration_test_support::verify_native_generation_case("before_selector");}

#[test]
#[serial_test::serial]
fn native_generation_selected_cleanup(){cc_switch_lib::integration_test_support::verify_native_generation_case("selected_cleanup");}

#[test]
#[serial_test::serial]
fn native_generation_partial_cleanup(){cc_switch_lib::integration_test_support::verify_native_generation_case("partial_cleanup");}

#[test]
#[serial_test::serial]
fn native_generation_cleanup_source_changed(){cc_switch_lib::integration_test_support::verify_native_generation_case("cleanup_source_changed");}

#[test]
#[serial_test::serial]
fn native_generation_startup_cleanup(){cc_switch_lib::integration_test_support::verify_native_generation_case("startup_cleanup");}

#[test]
#[serial_test::serial]
fn native_generation_pending_cache(){cc_switch_lib::integration_test_support::verify_native_generation_case("pending_cache");}

#[test]
#[serial_test::serial]
fn native_generation_pending_changes(){cc_switch_lib::integration_test_support::verify_native_generation_case("pending_changes");}

#[test]
#[serial_test::serial]
fn native_generation_empty_input(){cc_switch_lib::integration_test_support::verify_native_generation_case("empty_input");}

#[test]
#[serial_test::serial]
fn native_generation_dead_process_resume(){cc_switch_lib::integration_test_support::verify_native_generation_case("dead_process_resume");}
