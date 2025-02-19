//! # Global Environment Records
//!
//! A global Environment Record is used to represent the outer most scope that is shared by all
//! of the ECMAScript Script elements that are processed in a common realm.
//! A global Environment Record provides the bindings for built-in globals (clause 18),
//! properties of the global object, and for all top-level declarations (13.2.8, 13.2.10)
//! that occur within a Script.
//! More info:  <https://tc39.es/ecma262/#sec-global-environment-records>

use crate::{
    environment::{
        declarative_environment_record::DeclarativeEnvironmentRecord,
        environment_record_trait::EnvironmentRecordTrait,
        lexical_environment::{Environment, EnvironmentType, VariableScope},
        object_environment_record::ObjectEnvironmentRecord,
    },
    gc::{Finalize, Trace},
    object::JsObject,
    property::PropertyDescriptor,
    Context, JsResult, JsValue,
};
use gc::{Gc, GcCell};
use rustc_hash::FxHashSet;

#[derive(Debug, Trace, Finalize, Clone)]
pub struct GlobalEnvironmentRecord {
    pub object_record: ObjectEnvironmentRecord,
    pub global_this_binding: JsObject,
    pub declarative_record: DeclarativeEnvironmentRecord,
    pub var_names: GcCell<FxHashSet<Box<str>>>,
}

impl GlobalEnvironmentRecord {
    pub fn new(global: JsObject, this_value: JsObject) -> GlobalEnvironmentRecord {
        let obj_rec = ObjectEnvironmentRecord {
            bindings: global.into(),
            outer_env: None,
            /// Object Environment Records created for with statements (13.11)
            /// can provide their binding object as an implicit this value for use in function calls.
            /// The capability is controlled by a withEnvironment Boolean value that is associated
            /// with each object Environment Record. By default, the value of withEnvironment is false
            /// for any object Environment Record.
            with_environment: false,
        };

        let dcl_rec = DeclarativeEnvironmentRecord::new(None);

        GlobalEnvironmentRecord {
            object_record: obj_rec,
            global_this_binding: this_value,
            declarative_record: dcl_rec,
            var_names: GcCell::new(FxHashSet::default()),
        }
    }

    pub fn has_var_declaration(&self, name: &str) -> bool {
        self.var_names.borrow().contains(name)
    }

    pub fn has_lexical_declaration(&self, name: &str) -> bool {
        self.declarative_record.has_binding(name)
    }

    pub fn has_restricted_global_property(&self, name: &str) -> bool {
        let global_object = &self.object_record.bindings;
        let existing_prop = global_object.get_property(name);
        match existing_prop {
            Some(desc) => {
                if desc.expect_configurable() {
                    return false;
                }
                true
            }
            None => false,
        }
    }

    pub fn can_declare_global_var(&self, name: &str) -> bool {
        let global_object = &self.object_record.bindings;
        if global_object.has_field(name) {
            true
        } else {
            global_object.is_extensible()
        }
    }

    pub fn can_declare_global_function(&self, name: &str) -> bool {
        let global_object = &self.object_record.bindings;
        let existing_prop = global_object.get_property(name);
        match existing_prop {
            Some(prop) => {
                prop.expect_configurable()
                    || prop
                        .enumerable()
                        .zip(prop.writable())
                        .map_or(false, |(a, b)| a && b)
            }
            None => global_object.is_extensible(),
        }
    }

    pub fn create_global_var_binding(
        &mut self,
        name: String,
        deletion: bool,
        context: &mut Context,
    ) -> JsResult<()> {
        let obj_rec = &mut self.object_record;
        let global_object = &obj_rec.bindings;
        let has_property = global_object.has_field(name.as_str());
        let extensible = global_object.is_extensible();
        if !has_property && extensible {
            obj_rec.create_mutable_binding(name.clone(), deletion, false, context)?;
            obj_rec.initialize_binding(&name, JsValue::undefined(), context)?;
        }

        let mut var_declared_names = self.var_names.borrow_mut();
        if !var_declared_names.contains(name.as_str()) {
            var_declared_names.insert(name.into_boxed_str());
        }
        Ok(())
    }

    pub fn create_global_function_binding(&mut self, name: &str, value: JsValue, deletion: bool) {
        let global_object = &mut self.object_record.bindings;
        let existing_prop = global_object.get_property(name);
        // TODO: change to desc.is_undefined()
        let desc = match existing_prop {
            Some(desc) if desc.expect_configurable() => PropertyDescriptor::builder().value(value),
            Some(_) => PropertyDescriptor::builder()
                .value(value)
                .writable(true)
                .enumerable(true)
                .configurable(deletion),
            None => PropertyDescriptor::builder().value(value),
        };

        // TODO: fix spec by adding Set and append name to varDeclaredNames
        global_object
            .as_object()
            .expect("global object")
            .insert(name, desc);
    }
}

impl EnvironmentRecordTrait for GlobalEnvironmentRecord {
    fn has_binding(&self, name: &str) -> bool {
        if self.declarative_record.has_binding(name) {
            return true;
        }
        self.object_record.has_binding(name)
    }

    fn create_mutable_binding(
        &self,
        name: String,
        deletion: bool,
        allow_name_reuse: bool,
        context: &mut Context,
    ) -> JsResult<()> {
        if !allow_name_reuse && self.declarative_record.has_binding(&name) {
            return Err(
                context.construct_type_error(format!("Binding already exists for {}", name))
            );
        }

        self.declarative_record
            .create_mutable_binding(name, deletion, allow_name_reuse, context)
    }

    fn create_immutable_binding(
        &self,
        name: String,
        strict: bool,
        context: &mut Context,
    ) -> JsResult<()> {
        if self.declarative_record.has_binding(&name) {
            return Err(
                context.construct_type_error(format!("Binding already exists for {}", name))
            );
        }

        self.declarative_record
            .create_immutable_binding(name, strict, context)
    }

    fn initialize_binding(
        &self,
        name: &str,
        value: JsValue,
        context: &mut Context,
    ) -> JsResult<()> {
        if self.declarative_record.has_binding(name) {
            return self
                .declarative_record
                .initialize_binding(name, value, context);
        }

        assert!(
            self.object_record.has_binding(name),
            "Binding must be in object_record"
        );
        self.object_record.initialize_binding(name, value, context)
    }

    fn set_mutable_binding(
        &self,
        name: &str,
        value: JsValue,
        strict: bool,
        context: &mut Context,
    ) -> JsResult<()> {
        if self.declarative_record.has_binding(name) {
            return self
                .declarative_record
                .set_mutable_binding(name, value, strict, context);
        }
        self.object_record
            .set_mutable_binding(name, value, strict, context)
    }

    fn get_binding_value(
        &self,
        name: &str,
        strict: bool,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        if self.declarative_record.has_binding(name) {
            return self
                .declarative_record
                .get_binding_value(name, strict, context);
        }
        self.object_record.get_binding_value(name, strict, context)
    }

    fn delete_binding(&self, name: &str) -> bool {
        if self.declarative_record.has_binding(name) {
            return self.declarative_record.delete_binding(name);
        }

        let global: &JsValue = &self.object_record.bindings;
        if global.has_field(name) {
            let status = self.object_record.delete_binding(name);
            if status {
                let mut var_names = self.var_names.borrow_mut();
                if var_names.contains(name) {
                    var_names.remove(name);
                    return status;
                }
            }
        }
        true
    }

    fn has_this_binding(&self) -> bool {
        true
    }

    fn get_this_binding(&self, _context: &mut Context) -> JsResult<JsValue> {
        Ok(self.global_this_binding.clone().into())
    }

    fn has_super_binding(&self) -> bool {
        false
    }

    fn with_base_object(&self) -> Option<JsObject> {
        None
    }

    fn get_outer_environment(&self) -> Option<Environment> {
        None
    }

    fn get_outer_environment_ref(&self) -> Option<&Environment> {
        None
    }

    fn set_outer_environment(&mut self, _env: Environment) {
        // TODO: Implement
        todo!("Not implemented yet")
    }

    fn get_environment_type(&self) -> EnvironmentType {
        EnvironmentType::Global
    }

    fn recursive_create_mutable_binding(
        &self,
        name: String,
        deletion: bool,
        _scope: VariableScope,
        context: &mut Context,
    ) -> JsResult<()> {
        self.create_mutable_binding(name, deletion, false, context)
    }

    fn recursive_create_immutable_binding(
        &self,
        name: String,
        deletion: bool,
        _scope: VariableScope,
        context: &mut Context,
    ) -> JsResult<()> {
        self.create_immutable_binding(name, deletion, context)
    }

    fn recursive_set_mutable_binding(
        &self,
        name: &str,
        value: JsValue,
        strict: bool,
        context: &mut Context,
    ) -> JsResult<()> {
        self.set_mutable_binding(name, value, strict, context)
    }

    fn recursive_initialize_binding(
        &self,
        name: &str,
        value: JsValue,
        context: &mut Context,
    ) -> JsResult<()> {
        self.initialize_binding(name, value, context)
    }
}

impl From<GlobalEnvironmentRecord> for Environment {
    fn from(env: GlobalEnvironmentRecord) -> Environment {
        Gc::new(Box::new(env))
    }
}
