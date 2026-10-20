//! A compatible model must not require inputs the app's tool cannot supply.
use craft_ai_core::role::{eligible, fits};
use craft_ai_core::{
    Binding, CapabilityId, CapabilitySpec, Connection, ConnectionDraft, InputRole, ModelId,
    ModelInfo, Role, RoleId,
};

fn setup() -> (Role, Connection, Binding) {
    let capability = CapabilityId::parse("image.inpaint").unwrap();
    let role = Role {
        id: RoleId::new("source-only").unwrap(),
        label: "Source only".into(),
        capability: capability.clone(),
        inputs: vec![InputRole::Source],
    };
    let mut draft = ConnectionDraft::custom();
    draft.label = "Test".into();
    draft.endpoint = "https://service.example/v1".into();
    draft.uses_key = false;
    let mut connection = draft.prepare(&[]).unwrap().connection;
    connection.models = vec![
        ModelInfo::new(ModelId::new("needs-mask").unwrap(), "Needs mask").with(
            CapabilitySpec::new(capability)
                .accepting(InputRole::Source, 1)
                .accepting(InputRole::Mask, 1)
                .requiring(InputRole::Source)
                .requiring(InputRole::Mask),
        ),
    ];
    let binding = Binding {
        role: role.id.clone(),
        connection: connection.id.clone(),
        model: connection.models[0].id.clone(),
    };
    (role, connection, binding)
}

#[test]
fn picker_does_not_offer_a_model_needing_an_input_the_tool_cannot_supply() {
    let (role, connection, _) = setup();
    assert!(fits(&role, &connection.models[0]).is_none());
    assert!(eligible(&role, &[connection]).is_empty());
}

#[test]
fn binding_refuses_a_model_requiring_an_input_the_tool_cannot_supply() {
    let (role, connection, binding) = setup();
    let error = craft_ai_core::role::resolve(&role, &[&[binding]], &[connection]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "model needs-mask requires a 'mask' input that this tool does not provide"
    );
}

#[test]
fn supplied_requirements_and_optional_inputs_remain_compatible() {
    let (mut role, mut connection, binding) = setup();
    role.inputs.push(InputRole::Mask);
    assert!(fits(&role, &connection.models[0]).is_some());
    assert!(
        craft_ai_core::role::resolve(
            &role,
            &[std::slice::from_ref(&binding)],
            std::slice::from_ref(&connection)
        )
        .is_ok()
    );
    role.inputs = vec![InputRole::Source];
    connection.models[0].capabilities[0].requires = vec![InputRole::Source];
    assert!(fits(&role, &connection.models[0]).is_some());
    assert!(craft_ai_core::role::resolve(&role, &[&[binding]], &[connection]).is_ok());
}
