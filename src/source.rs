use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs,
    path::PathBuf,
};

use naga_oil::compose::{ComposableModuleDescriptor, Composer};

use crate::{
    exports::{strip_exports, Export},
    files::{AbsoluteRustFilePathBuf, AbsoluteRustRootPathBuf, AbsoluteWGSLFilePathBuf},
    imports::ImportOrder,
    result::ShaderResult,
    Constants, MacroInput,
};

/// Shader sourcecode generated from the token stream provided
pub(crate) struct Sourcecode {
    exports: HashSet<Export>,
    requested_path_input: String,
    source_path: AbsoluteWGSLFilePathBuf,
    invocation_path: AbsoluteRustFilePathBuf,
    project_root: Option<AbsoluteRustRootPathBuf>,
    errors: Vec<String>,
    dependents: Vec<AbsoluteWGSLFilePathBuf>,
    includes: HashMap<String, (Vec<String>, PathBuf, String)>,
    constants: Constants,
}

impl Sourcecode {
    pub(crate) fn new(invocation_path: AbsoluteRustFilePathBuf, ins: MacroInput) -> Self {
        let MacroInput {
            wgsl_path: requested_path_input,
            includes,
            constants,
        } = ins;

        // Interpret as relative to invoking file
        let source_path = invocation_path
            .parent()
            .expect("files have parent directories")
            .join(&requested_path_input);
        if !source_path.is_file() {
            if source_path.exists() {
                panic!(
                    "could not find import `{}`: `{}` exists but is not a file",
                    requested_path_input,
                    source_path.display()
                )
            }
            panic!(
                "could not find import `{}`: `{}` does not exist",
                requested_path_input,
                source_path.display()
            );
        }
        assert!(source_path.is_absolute());

        if source_path.extension() != Some(OsStr::new("wgsl")) {
            panic!(
                "file `{}` does not have the required `.wgsl` extension",
                requested_path_input,
            );
        };

        let source_path = AbsoluteWGSLFilePathBuf::new(source_path);

        // Calculate top level exports
        let root_src = std::fs::read_to_string(&*source_path).expect("asserted was file");
        let (_, exports) = strip_exports(&root_src);

        let project_root = invocation_path.get_source_rust_root();

        Self {
            requested_path_input,
            source_path,
            invocation_path,
            project_root,
            exports,
            errors: Vec::new(),
            dependents: Vec::new(),
            includes,
            constants,
        }
    }

    /// Traverses the imports in each file, starting with the file given by this object, to give all of the files required
    /// and the order in which they need to be processed.
    fn find_import_order(&mut self) -> Option<ImportOrder> {
        match ImportOrder::calculate(self.source_path.clone(), self.project_root.as_ref()) {
            Ok(order) => Some(order),
            Err(err) => {
                self.push_error(format!("{}", err));
                None
            }
        }
    }

    /// Uses naga_oil to process includes
    fn compose(&mut self) -> Option<naga::Module> {
        let mut composer = Composer::default();
        composer.capabilities = naga::valid::Capabilities::all();
        composer.validate = true;

        let mut shader_defs = HashMap::new();
        if cfg!(debug_assertions) {
            shader_defs.insert(
                "__DEBUG".to_string(),
                naga_oil::compose::ShaderDefValue::Bool(true),
            );
        }

        for (a, b) in &self.constants.inner {
            shader_defs.insert(
                a.clone(),
                naga_oil::compose::ShaderDefValue::from(b.clone()),
            );
        }

        let (_, reqs, _) = naga_oil::compose::get_preprocessor_data(
            fs::read_to_string(self.requested_path()).ok()?.as_str(),
        );

        let mut reqs = reqs
            .into_iter()
            .map(|req| req.import)
            .collect::<HashSet<_>>();

        while !reqs.is_empty() {
            let mut next_reqs = HashSet::default();

            for (req, (subreqs, path, src)) in reqs
                .iter()
                .filter_map(|r| self.includes.get(r).map(|n| (r, n)))
            {
                if composer.contains_module(req) {
                    continue;
                }

                if subreqs.iter().all(|sr| composer.contains_module(&sr)) {
                    composer
                        .add_composable_module(ComposableModuleDescriptor {
                            source: &src,
                            file_path: &path.to_string_lossy(),
                            language: naga_oil::compose::ShaderLanguage::Wgsl,
                            as_name: Some(req.clone()),
                            ..Default::default()
                        })
                        .unwrap();
                }
                next_reqs.extend(
                    subreqs
                        .iter()
                        .cloned()
                        .filter(|r| !composer.contains_module(r)),
                );
                next_reqs.insert(req.clone());
            }

            reqs = next_reqs;
        }

        // Calculate import order
        let import_order = self.find_import_order()?;

        // Calculate names of imports
        let reduced_names = import_order.reduced_names();

        // Add imports in order to naga-oil
        let (imports, root) = import_order.modules();
        for import in imports {
            self.dependents.push(import.path());

            let desc = import.to_composable_module_descriptor(
                &reduced_names,
                self.project_root.as_ref(),
                shader_defs.clone(),
            );
            let desc = match desc {
                Ok(desc) => desc,
                Err(errors) => {
                    for error in errors {
                        self.push_error(error);
                    }
                    return None;
                }
            };

            let res = composer.add_composable_module(desc.borrow_composable_descriptor());
            if let Err(e) = res {
                self.push_error(crate::error::format_compose_error(e, &composer));
            }
        }

        if !self.errors.is_empty() {
            return None;
        }

        // Add main module to link everything
        let desc =
            root.to_naga_module_descriptor(&reduced_names, self.project_root.as_ref(), shader_defs);
        let desc = match desc {
            Ok(desc) => desc,
            Err(errors) => {
                for error in errors {
                    self.push_error(error);
                }
                return None;
            }
        };
        let res = composer.make_naga_module(desc.borrow_module_descriptor());

        match res {
            Ok(module) => Some(module),
            Err(e) => {
                self.push_error(crate::error::format_compose_error(e, &composer));

                None
            }
        }
    }

    pub(crate) fn complete(mut self) -> ShaderResult {
        let module = self.compose().unwrap_or_default();

        ShaderResult::new(self, module)
    }

    pub(crate) fn push_error(&mut self, message: String) {
        self.errors.push(message)
    }

    pub(crate) fn errors(&self) -> impl Iterator<Item = &String> {
        self.errors.iter()
    }

    pub(crate) fn dependents(&self) -> impl Iterator<Item = &AbsoluteWGSLFilePathBuf> {
        self.dependents.iter()
    }

    pub(crate) fn requested_path(&self) -> &str {
        &self.requested_path_input
    }

    pub(crate) fn invocation_path(&self) -> &AbsoluteRustFilePathBuf {
        &self.invocation_path
    }

    pub(crate) fn exports(&self) -> &HashSet<Export> {
        &self.exports
    }
}
