use super::imports::{ImportSpec, ModuleFuncImports, SigType, StaticSignature};
use cranelift_codegen::ir;
use cranelift_codegen::isa::{TargetFrontendConfig, TargetIsa};
use cranelift_jit::JITModule;
use cranelift_module::{DataId, FuncId, Linkage, Module};

pub(super) struct FuncBuildImports<'a> {
    module_imports: &'a mut ModuleFuncImports,
    func_refs_by_internal_id: Vec<Option<ir::FuncRef>>,
}

impl<'a> FuncBuildImports<'a> {
    pub(super) fn new(module_imports: &'a mut ModuleFuncImports) -> Self {
        Self {
            module_imports,
            func_refs_by_internal_id: Vec::new(),
        }
    }

    pub(super) fn get(
        &mut self,
        codegen_env: &mut impl JitCodegenEnv,
        func: &mut ir::Function,
        spec: &'static ImportSpec,
    ) -> Result<ir::FuncRef, String> {
        let internal_id = spec.internal_id();
        if internal_id >= self.func_refs_by_internal_id.len() {
            self.func_refs_by_internal_id.resize(internal_id + 1, None);
        }
        if let Some(func_ref) = self.func_refs_by_internal_id[internal_id] {
            return Ok(func_ref);
        }
        let func_id = self.module_imports.ensure_declared(codegen_env, spec)?;
        let func_ref = codegen_env.codegen_declare_func_in_func(func_id, func)?;
        self.func_refs_by_internal_id[internal_id] = Some(func_ref);
        Ok(func_ref)
    }

    pub(super) fn get_or_panic(
        &mut self,
        codegen_env: &mut impl JitCodegenEnv,
        func: &mut ir::Function,
        spec: &'static ImportSpec,
    ) -> ir::FuncRef {
        self.get(codegen_env, func, spec).unwrap_or_else(|err| {
            panic!(
                "failed to bind import {} during JIT codegen: {}",
                spec.symbol, err
            )
        })
    }
}

pub(super) trait JitCodegenEnv {
    fn codegen_isa(&self) -> &dyn TargetIsa;

    fn codegen_jit_module_mut(&mut self) -> Option<&mut JITModule> {
        None
    }

    fn function_declaration(&self, id: FuncId) -> Result<(&ir::Signature, Linkage), String>;

    fn data_declaration(&self, id: DataId) -> Result<(Linkage, bool), String>;

    fn codegen_declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        signature: &ir::Signature,
    ) -> Result<FuncId, String>;

    fn codegen_declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> Result<DataId, String>;

    fn codegen_target_config(&self) -> TargetFrontendConfig {
        self.codegen_isa().frontend_config()
    }

    fn codegen_make_context(&self) -> cranelift_codegen::Context {
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func.signature.call_conv = self.codegen_isa().default_call_conv();
        ctx
    }

    fn codegen_clear_context(&self, ctx: &mut cranelift_codegen::Context) {
        ctx.clear();
        ctx.func.signature.call_conv = self.codegen_isa().default_call_conv();
    }

    fn codegen_make_signature(&self) -> ir::Signature {
        ir::Signature::new(self.codegen_isa().default_call_conv())
    }

    fn codegen_declare_func_in_func(
        &mut self,
        func_id: FuncId,
        func: &mut ir::Function,
    ) -> Result<ir::FuncRef, String> {
        let (signature, linkage) = self.function_declaration(func_id)?;
        let signature = func.import_signature(signature.clone());
        let user_name_ref = func.declare_imported_user_function(ir::UserExternalName {
            namespace: 0,
            index: func_id.as_u32(),
        });
        Ok(func.import_function(ir::ExtFuncData {
            name: ir::ExternalName::user(user_name_ref),
            signature,
            colocated: linkage.is_final(),
            patchable: false,
        }))
    }

    fn codegen_declare_data_in_func(
        &mut self,
        data_id: DataId,
        func: &mut ir::Function,
    ) -> Result<ir::GlobalValue, String> {
        let (linkage, tls) = self.data_declaration(data_id)?;
        let user_name_ref = func.declare_imported_user_function(ir::UserExternalName {
            namespace: 1,
            index: data_id.as_u32(),
        });
        Ok(func.create_global_value(ir::GlobalValueData::Symbol {
            name: ir::ExternalName::user(user_name_ref),
            offset: ir::immediates::Imm64::new(0),
            colocated: linkage.is_final(),
            tls,
        }))
    }
}

impl JitCodegenEnv for JITModule {
    fn codegen_isa(&self) -> &dyn TargetIsa {
        Module::isa(self)
    }

    fn codegen_jit_module_mut(&mut self) -> Option<&mut JITModule> {
        Some(self)
    }

    fn function_declaration(&self, id: FuncId) -> Result<(&ir::Signature, Linkage), String> {
        let declaration = self.declarations().get_function_decl(id);
        Ok((&declaration.signature, declaration.linkage))
    }

    fn data_declaration(&self, id: DataId) -> Result<(Linkage, bool), String> {
        let declaration = self.declarations().get_data_decl(id);
        Ok((declaration.linkage, declaration.tls))
    }

    fn codegen_declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        signature: &ir::Signature,
    ) -> Result<FuncId, String> {
        Module::declare_function(self, name, linkage, signature)
            .map_err(|err| format!("failed to declare JIT function {name}: {err}"))
    }

    fn codegen_declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> Result<DataId, String> {
        Module::declare_data(self, name, linkage, writable, tls)
            .map_err(|err| format!("failed to declare JIT data {name}: {err}"))
    }
}

pub(super) fn lower_static_signature(
    codegen_env: &impl JitCodegenEnv,
    signature: StaticSignature,
) -> ir::Signature {
    let mut lowered = codegen_env.codegen_make_signature();
    let lower_sig_type = |sig_type| match sig_type {
        SigType::Pointer => codegen_env.codegen_target_config().pointer_type(),
        SigType::I64 => ir::types::I64,
        SigType::I32 => ir::types::I32,
    };
    for param in signature.params {
        lowered
            .params
            .push(ir::AbiParam::new(lower_sig_type(*param)));
    }
    for ret in signature.returns {
        lowered
            .returns
            .push(ir::AbiParam::new(lower_sig_type(*ret)));
    }
    lowered
}

pub(super) fn declare_import_fn(
    codegen_env: &mut impl JitCodegenEnv,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    codegen_env
        .codegen_declare_function(symbol, Linkage::Import, sig)
        .map_err(|err| format!("failed to declare imported {symbol} symbol: {err}"))
}

pub(super) fn declare_local_fn(
    codegen_env: &mut impl JitCodegenEnv,
    symbol: &str,
    sig: &ir::Signature,
) -> Result<FuncId, String> {
    codegen_env
        .codegen_declare_function(symbol, Linkage::Local, sig)
        .map_err(|err| format!("failed to declare local {symbol} function: {err}"))
}
