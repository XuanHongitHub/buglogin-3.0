use buglogin_api::{
    commands::{Adapter, CommandRegistry},
    http::rest_operation_ids,
};

#[test]
fn rest_registry_has_stable_operation_ids() {
    let expected: Vec<_> = CommandRegistry::all()
        .iter()
        .filter(|command| command.adapters.contains(&Adapter::Rest))
        .map(|command| command.id.as_str())
        .collect();
    assert_eq!(rest_operation_ids(), expected);
    assert!(rest_operation_ids().contains(&"proxy.parse"));
}
