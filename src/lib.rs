#![doc=include_str!("../readme.md")]

use std::{
    cell::{
        RefCell,
    },
    collections::{
        BTreeMap,
        HashMap,
    },
    fs::{
        self,
        create_dir_all,
    },
    io::{
        self,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
    process::{
        self,
        Command,
        Stdio,
    },
    rc::Rc,
    str::FromStr,
    marker::PhantomData,
};
use serde::{
    de::DeserializeOwned,
    Deserialize,
    Serialize,
};
use serde_json::{
    json,
    Value,
};
use thiserror::Error;

pub(crate) mod utils;
pub mod ref_;
pub mod expr;
pub mod func;
pub mod list_field;
pub mod list_ref;
pub mod rec_field;
pub mod rec_ref;
pub mod output;
pub mod prim_field;
pub mod prim_ref;
pub mod set_field;
pub mod set_ref;
pub mod variable;
pub mod helpers;

pub use ref_::*;
pub use expr::*;
pub use func::*;
pub use list_field::*;
pub use list_ref::*;
pub use rec_field::*;
pub use rec_ref::*;
pub use output::*;
pub use prim_field::*;
pub use prim_ref::*;
pub use set_field::*;
pub use set_ref::*;
use utils::REPLACE_EXPRS;
pub use variable::*;
pub use helpers::*;

/// Use this to create a new stack.
pub struct BuildStack {}

impl BuildStack {
    pub fn build(self) -> Stack {
        return Stack {
            providers: Default::default(),
            variables: Default::default(),
            datasources: Default::default(),
            resources: Default::default(),
            outputs: Default::default(),
            shared: StackShared(Rc::new(RefCell::new(StackShared_ { replace_exprs: Default::default() }))),
        };
    }
}

#[derive(Debug)]
pub enum ComponentType {
    ProviderType,
    Provider,
    Variable,
    Datasource,
    Resource,
    Output,
}

#[derive(Error, Debug)]
pub enum StackError {
    #[error("Duplicate {0:?} with tf_id {1}")]
    Duplicate(ComponentType, String),
}

#[derive(Error, Debug)]
pub enum RunError {
    #[error("Failed to prepare run directory {0:?}: {1:?}")]
    FsError(PathBuf, io::Error),
    #[error("Error serializing stack: {0:?}")]
    StackError(
        #[from]
        StackError,
    ),
    #[error("Failed to write configs: {0:?}")]
    FileError(
        #[from]
        io::Error,
    ),
    #[error("Failed to write or parse json: {0:?}")]
    JsonError(
        #[from]
        serde_json::Error,
    ),
    #[error("Command {0:?} failed with result {1:?}")]
    CommandError(Command, process::ExitStatus),
}

struct StackShared_ {
    replace_exprs: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct StackShared(Rc<RefCell<StackShared_>>);

impl StackShared {
    pub fn add_sentinel(&self, v: &str) -> String {
        let mut m = self.0.borrow_mut();
        let k = format!("_TERRARS_SENTINEL_{}_", m.replace_exprs.len());
        m.replace_exprs.push((k.clone(), format!("${{{}}}", v)));
        k
    }
}

pub struct Stack {
    providers: Vec<Rc<dyn Provider>>,
    variables: Vec<Rc<dyn VariableTrait>>,
    datasources: Vec<Rc<dyn Datasource_>>,
    resources: Vec<Rc<dyn Resource_>>,
    outputs: Vec<Rc<dyn Output>>,
    pub shared: StackShared,
}

impl Stack {
    /// Turn a value into into an expression that evaluates to that value (ex:
    /// `expr_lit(44)` or `expr_lit("hi")`) for use in other expressions, like
    /// Terraform function calls. NOTE: Converting from an expression to a string then
    /// back to an expression again will result in double escaping (broken SENTINEL
    /// junk).
    pub fn expr_lit<T: PrimType>(&self, expr: T) -> PrimExpr<T> {
        PrimExpr(self.shared.clone(), expr.to_expr_raw(), Default::default())
    }

    /// Turn a raw expression string into a `PrimExpr` - the string must be properly
    /// escaped, etc.
    pub fn expr<T: PrimType>(&self, expr: impl ToString) -> PrimExpr<T> {
        PrimExpr(self.shared.clone(), expr.to_string(), Default::default())
    }

    /// Start a new function call expression
    pub fn func(&self, name: &str) -> Func {
        Func {
            shared: self.shared.clone(),
            data: format!("{}(", name),
            first: true,
        }
    }

    /// Convert the stack to json bytes.
    pub fn serialize(&self, state_path: &Path) -> Result<Vec<u8>, StackError> {
        REPLACE_EXPRS.with(move |f| {
            *f.borrow_mut() = Some(self.shared.0.borrow().replace_exprs.clone());
        });
        let mut required_providers = BTreeMap::new();
        for p in &self.providers {
            match required_providers.entry(p.extract_type_tf_id()) {
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert(p.extract_provider_type());
                },
                std::collections::btree_map::Entry::Occupied(_) => { },
            };
        }
        let mut providers = BTreeMap::new();
        for p in &self.providers {
            providers.entry(p.extract_type_tf_id()).or_insert_with(Vec::new).push(p.extract_provider());
        }
        let mut variables = BTreeMap::new();
        for v in &self.variables {
            if variables.insert(v.extract_tf_id(), v.extract_value()).is_some() {
                Err(StackError::Duplicate(ComponentType::Variable, v.extract_tf_id()))?;
            }
        }
        let mut data = BTreeMap::new();
        for d in &self.datasources {
            if data
                .entry(d.extract_datasource_type())
                .or_insert_with(BTreeMap::new)
                .insert(d.extract_tf_id(), d.extract_value())
                .is_some() {
                Err(StackError::Duplicate(ComponentType::Datasource, d.extract_tf_id()))?;
            }
        }
        let mut resources = BTreeMap::new();
        for r in &self.resources {
            if resources
                .entry(r.extract_resource_type())
                .or_insert_with(BTreeMap::new)
                .insert(r.extract_tf_id(), r.extract_value())
                .is_some() {
                Err(StackError::Duplicate(ComponentType::Resource, r.extract_tf_id()))?;
            }
        }
        let mut outputs = BTreeMap::new();
        for o in &self.outputs {
            if outputs.insert(o.extract_tf_id(), o.extract_value()).is_some() {
                Err(StackError::Duplicate(ComponentType::Output, o.extract_tf_id()))?;
            }
        }
        let mut out = BTreeMap::new();
        out.insert("terraform", json!({
            "backend": {
                "local": {
                    "path": state_path.to_string_lossy(),
                },
            },
            "required_providers": required_providers,
        }));
        if !providers.is_empty() {
            out.insert("provider", json!(providers));
        }
        if !variables.is_empty() {
            out.insert("variable", json!(variables));
        }
        if !data.is_empty() {
            out.insert("data", json!(data));
        }
        if !resources.is_empty() {
            out.insert("resource", json!(resources));
        }
        if !outputs.is_empty() {
            out.insert("output", json!(outputs));
        }
        REPLACE_EXPRS.with(|f| *f.borrow_mut() = None);
        let res = serde_json::to_vec_pretty(&out).unwrap();
        Ok(res)
    }

    pub fn add_provider(&mut self, v: Rc<dyn Provider>) {
        self.providers.push(v);
    }

    pub fn add_datasource(&mut self, v: Rc<dyn Datasource_>) {
        self.datasources.push(v);
    }

    pub fn add_resource(&mut self, v: Rc<dyn Resource_>) {
        self.resources.push(v);
    }

    /// Serialize the stack to a file and run a Terraform command on it. If variables
    /// are provided, they must be a single-level struct where all values are
    /// primitives (i64, f64, String, bool).
    pub fn run<V: Serialize>(&self, path: &Path, variables: Option<&V>, mode: &str) -> Result<(), RunError> {
        create_dir_all(path).map_err(|e| RunError::FsError(path.to_path_buf(), e))?;
        let state_name = "state.tfstate";
        fs::write(&path.join("stack.tf.json"), &self.serialize(&PathBuf::from_str(state_name).unwrap())?)?;
        let state_path = path.join(state_name);
        if !state_path.exists() {
            let mut command = Command::new(get_terraform_binary());
            command.current_dir(&path).arg("init");
            let res = command.status()?;
            if !res.success() {
                return Err(RunError::CommandError(command, res));
            }
        }
        let mut command = Command::new(get_terraform_binary());
        command.current_dir(&path).arg(mode);
        if let Some(vars) = variables {
            let mut vars_file = tempfile::Builder::new().suffix(".json").tempfile()?;
            vars_file.as_file_mut().write_all(&serde_json::to_vec_pretty(&vars)?)?;
            command.arg(format!("-var-file={}", vars_file.path().to_string_lossy()));
            let res = command.status()?;
            if !res.success() {
                return Err(RunError::CommandError(command, res))?;
            }
        } else {
            let res = command.status()?;
            if !res.success() {
                return Err(RunError::CommandError(command, res))?;
            }
        }
        Ok(())
    }

    /// Gets the current outputs from an applied stack. `path` is the directory in
    /// which the .tf.json file was written. The output struct must be a single level
    /// and only have primitive values (i64, f64, String, bool).
    pub fn get_output<O: DeserializeOwned>(&self, path: &Path) -> Result<O, RunError> {
        let mut command = Command::new(get_terraform_binary());
        let res = command.current_dir(&path).stderr(Stdio::inherit()).args(&["output", "-json"]).output()?;
        if !res.status.success() {
            return Err(RunError::CommandError(command, res.status));
        }

        // Redeserialize... hack
        #[derive(Deserialize)]
        struct Var {
            value: Value,
        }

        Ok(
            serde_json::from_slice(
                &serde_json::to_vec(
                    &serde_json::from_slice::<HashMap<String, Var>>(&res.stdout)?
                        .into_iter()
                        .map(|(k, v)| (k, v.value))
                        .collect::<HashMap<String, Value>>(),
                )?,
            )?,
        )
    }
}

// Generated traits
pub trait Referable {
    fn extract_ref(&self) -> String;
}

pub trait Provider {
    fn extract_type_tf_id(&self) -> String;
    fn extract_provider_type(&self) -> Value;
    fn extract_provider(&self) -> Value;
}

pub trait Datasource: Referable { }

pub trait Datasource_ {
    fn extract_datasource_type(&self) -> String;
    fn extract_tf_id(&self) -> String;
    fn extract_value(&self) -> Value;
}

pub trait Resource: Referable { }

pub trait Resource_ {
    fn extract_resource_type(&self) -> String;
    fn extract_tf_id(&self) -> String;
    fn extract_value(&self) -> Value;
}

// Provider extras
#[derive(Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IgnoreChangesAll {
    All,
}

#[derive(Serialize, PartialEq)]
#[serde(untagged)]
pub enum IgnoreChanges {
    All(IgnoreChangesAll),
    Refs(Vec<String>),
}

#[derive(Serialize, Default, PartialEq)]
pub struct ResourceLifecycle {
    pub create_before_destroy: bool,
    pub prevent_destroy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_changes: Option<IgnoreChanges>,
    pub replace_triggered_by: Vec<String>,
}

#[derive(Serialize)]
pub struct DynamicBlock<T: Serialize> {
    pub for_each: String,
    pub iterator: String,
    pub content: T,
}

pub enum BlockAssignable<T: Serialize> {
    Literal(Vec<T>),
    Dynamic(DynamicBlock<T>),
}

impl<T: Serialize> From<Vec<T>> for BlockAssignable<T> {
    fn from(value: Vec<T>) -> Self {
        BlockAssignable::Literal(value)
    }
}

impl<T: Serialize> From<DynamicBlock<T>> for BlockAssignable<T> {
    fn from(value: DynamicBlock<T>) -> Self {
        BlockAssignable::Dynamic(value)
    }
}

pub trait SerdeSkipDefault {
    fn is_default(&self) -> bool;
    fn is_not_default(&self) -> bool;
}

impl<T: Default + PartialEq> SerdeSkipDefault for T {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    fn is_not_default(&self) -> bool {
        !self.is_default()
    }
}

pub struct MapKV<T: Ref> {
    pub(crate) shared: StackShared,
    pub(crate) _pd: PhantomData<T>,
}

impl<T: Ref> MapKV<T> {
    pub(crate) fn new(shared: StackShared) -> Self {
        Self {
            shared: shared,
            _pd: Default::default(),
        }
    }

    pub fn key(&self) -> PrimExpr<String> {
        PrimExpr::new(self.shared.clone(), "each.key".into())
    }

    pub fn value(&self) -> T {
        T::new(self.shared.clone(), "each.value".into())
    }
}
