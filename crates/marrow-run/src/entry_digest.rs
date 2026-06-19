use marrow_check::{
    CheckedArg, CheckedBinaryOp, CheckedBody, CheckedBuiltinCall, CheckedCallTarget,
    CheckedCatchClause, CheckedElseIf, CheckedEnumMemberRef, CheckedEnumRef, CheckedExpr,
    CheckedForBinding, CheckedFunctionRef, CheckedIdentityConstructor, CheckedInterpolationPart,
    CheckedLiteralKind, CheckedMatchArm, CheckedResourceConstructor,
    CheckedResourceConstructorField, CheckedResourceRef, CheckedRuntimeFunction,
    CheckedRuntimeProgram, CheckedRuntimeValueType, CheckedSavedIndex, CheckedSavedIndexKey,
    CheckedSavedKeyParam, CheckedSavedLayer, CheckedSavedMember, CheckedSavedMemberKind,
    CheckedSavedPlace, CheckedSavedTerminal, CheckedStdCall, CheckedStmt, CheckedUnaryOp,
    MarrowType, StoreIndexKeySource, StoreLeafKind, StoredValueMeaning, Type,
};
use marrow_schema::ReturnPresence;
use marrow_schema::stdlib::Capability;
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;
use sha2::{Digest, Sha256};

use crate::call::function_by_ref;
use crate::entry::canonical_entry_name;

pub(super) fn entry_digest(
    program: &CheckedRuntimeProgram,
    module: &marrow_check::CheckedRuntimeModule,
    function: &CheckedRuntimeFunction,
) -> String {
    let mut digest = EntryDigest::new(program);
    digest.entry(module, function);
    sha256_digest(&digest.finish())
}

fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", crate::hex::encode(digest.as_ref()))
}

struct EntryDigest<'p> {
    program: &'p CheckedRuntimeProgram,
    bytes: Vec<u8>,
}

impl<'p> EntryDigest<'p> {
    fn new(program: &'p CheckedRuntimeProgram) -> Self {
        Self {
            program,
            bytes: Vec::new(),
        }
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn entry(
        &mut self,
        module: &marrow_check::CheckedRuntimeModule,
        function: &CheckedRuntimeFunction,
    ) {
        self.atom("marrow-entry-v2");
        self.atom(&canonical_entry_name(module, function));
        self.return_presence(function.return_presence);
        self.option_type(function.return_type.as_ref());
        self.seq(function.entry_params(), |this, param| {
            this.atom(&param.name);
            this.runtime_type(&param.ty);
        });
        self.option_body(function.body());
    }

    fn atom(&mut self, value: &str) {
        self.bytes
            .extend_from_slice(value.len().to_string().as_bytes());
        self.bytes.push(b':');
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(b';');
    }

    fn bool(&mut self, value: bool) {
        self.atom(if value { "true" } else { "false" });
    }

    fn usize(&mut self, value: usize) {
        self.atom(&value.to_string());
    }

    fn seq<T>(&mut self, values: &[T], mut render: impl FnMut(&mut Self, &T)) {
        self.usize(values.len());
        for value in values {
            render(self, value);
        }
    }

    fn option<T>(&mut self, value: Option<&T>, render: impl FnOnce(&mut Self, &T)) {
        match value {
            Some(value) => {
                self.atom("some");
                render(self, value);
            }
            None => self.atom("none"),
        }
    }

    fn option_body(&mut self, body: Option<&CheckedBody>) {
        self.option(body, Self::body);
    }

    fn option_expr(&mut self, expr: Option<&CheckedExpr>) {
        self.option(expr, Self::expr);
    }

    fn option_type(&mut self, ty: Option<&MarrowType>) {
        self.option(ty, Self::marrow_type);
    }

    fn body(&mut self, body: &CheckedBody) {
        self.atom("body");
        self.seq(body.statements(), Self::stmt);
    }

    fn stmt(&mut self, stmt: &CheckedStmt) {
        match stmt {
            CheckedStmt::Const {
                name,
                binding_type,
                value,
                ..
            } => {
                self.atom("const");
                self.atom(name);
                self.option_type(binding_type.as_ref());
                self.expr(value);
            }
            CheckedStmt::Var {
                name,
                key_count,
                ty,
                binding_type,
                resource_default,
                value,
                ..
            } => {
                self.atom("var");
                self.atom(name);
                self.usize(*key_count);
                self.option(ty.as_ref(), Self::schema_type);
                self.option_type(binding_type.as_ref());
                self.bool(*resource_default);
                self.option_expr(value.as_ref());
            }
            CheckedStmt::Assign { target, value, .. } => {
                self.atom("assign");
                self.expr(target);
                self.expr(value);
            }
            CheckedStmt::Delete { path, .. } => {
                self.atom("delete");
                self.expr(path);
            }
            CheckedStmt::Return { value, .. } => {
                self.atom("return");
                self.option_expr(value.as_ref());
            }
            CheckedStmt::ReturnAbsent { .. } => self.atom("return_absent"),
            CheckedStmt::Break { .. } => self.atom("break"),
            CheckedStmt::Continue { .. } => self.atom("continue"),
            CheckedStmt::Throw { value, .. } => {
                self.atom("throw");
                self.expr(value);
            }
            CheckedStmt::Expr { value, .. } => {
                self.atom("expr_stmt");
                self.expr(value);
            }
            CheckedStmt::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                self.atom("if");
                self.option_expr(condition.as_ref());
                self.body(then_block);
                self.seq(else_ifs, Self::else_if);
                self.option(else_block.as_ref(), Self::body);
            }
            CheckedStmt::IfConst {
                name,
                binding_type,
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                self.atom("if_const");
                self.atom(name);
                self.option_type(binding_type.as_ref());
                self.expr(value);
                self.body(then_block);
                self.seq(else_ifs, Self::else_if);
                self.option(else_block.as_ref(), Self::body);
            }
            CheckedStmt::While {
                condition, body, ..
            } => {
                self.atom("while");
                self.option_expr(condition.as_ref());
                self.body(body);
            }
            CheckedStmt::For {
                binding,
                iterable,
                step,
                body,
                ..
            } => {
                self.atom("for");
                self.for_binding(binding);
                self.expr(iterable);
                self.option(step.as_ref(), Self::expr);
                self.body(body);
            }
            CheckedStmt::Transaction { body, .. } => {
                self.atom("transaction");
                self.body(body);
            }
            CheckedStmt::Try { body, catch, .. } => {
                self.atom("try");
                self.body(body);
                self.option(catch.as_ref(), Self::catch_clause);
            }
            CheckedStmt::Match {
                scrutinee,
                arms,
                enum_ref,
                ..
            } => {
                self.atom("match");
                self.option_expr(scrutinee.as_ref());
                self.seq(arms, Self::match_arm);
                self.option(enum_ref.as_ref(), Self::enum_ref);
            }
        }
    }

    fn else_if(&mut self, else_if: &CheckedElseIf) {
        self.atom("else_if");
        self.option_expr(else_if.condition.as_ref());
        self.body(&else_if.block);
    }

    fn catch_clause(&mut self, catch: &CheckedCatchClause) {
        self.atom("catch");
        self.atom(&catch.name);
        self.body(&catch.block);
    }

    fn match_arm(&mut self, arm: &CheckedMatchArm) {
        self.atom("arm");
        self.seq(&arm.path, |this, segment| this.atom(segment));
        let member = arm
            .member_id
            .and_then(|id| self.program.facts().enum_member(id));
        self.option(member, |this, member| {
            this.catalog_or_name(member.catalog_id.as_deref(), &member.name);
        });
        self.seq(&arm.member_uses, |this, (member_id, _span)| {
            this.option(
                this.program.facts().enum_member(*member_id),
                |this, member| {
                    this.catalog_or_name(member.catalog_id.as_deref(), &member.name);
                },
            );
        });
        self.body(&arm.block);
    }

    fn for_binding(&mut self, binding: &CheckedForBinding) {
        self.atom("for_binding");
        self.atom(&binding.first);
        self.option(binding.second.as_ref(), |this, second| this.atom(second));
    }

    fn expr(&mut self, expr: &CheckedExpr) {
        match expr {
            CheckedExpr::Literal { kind, text, .. } => {
                self.atom("literal");
                self.literal_kind(*kind);
                self.atom(text);
            }
            CheckedExpr::Name {
                segments,
                enum_member,
                ..
            } => {
                self.atom("name");
                self.seq(segments, |this, segment| this.atom(segment));
                self.option(enum_member.as_ref(), Self::enum_member_ref);
            }
            CheckedExpr::SavedRoot { name, place, .. } => {
                self.atom("saved_root");
                self.atom(name);
                self.option(place.as_ref(), Self::saved_place);
            }
            CheckedExpr::Call {
                callee,
                args,
                target,
                place,
                ..
            } => {
                self.atom("call");
                self.expr(callee);
                self.seq(args, Self::arg);
                self.call_target(target);
                self.option(place.as_ref(), Self::saved_place);
            }
            CheckedExpr::Field {
                base,
                name,
                quoted,
                place,
                ..
            } => {
                self.atom("field");
                self.expr(base);
                self.atom(name);
                self.bool(*quoted);
                self.option(place.as_ref(), Self::saved_place);
            }
            CheckedExpr::OptionalField {
                base,
                name,
                quoted,
                place,
                ..
            } => {
                self.atom("optional_field");
                self.expr(base);
                self.atom(name);
                self.bool(*quoted);
                self.option(place.as_ref(), Self::saved_place);
            }
            CheckedExpr::Unary { op, operand, .. } => {
                self.atom("unary");
                self.unary_op(*op);
                self.expr(operand);
            }
            CheckedExpr::Binary {
                op, left, right, ..
            } => {
                self.atom("binary");
                self.binary_op(*op);
                self.expr(left);
                self.expr(right);
            }
            CheckedExpr::Range {
                start,
                end,
                inclusive_end,
                step,
                ..
            } => {
                self.atom("range");
                self.option(start.as_deref(), Self::expr);
                self.option(end.as_deref(), Self::expr);
                self.bool(*inclusive_end);
                self.option(step.as_deref(), Self::expr);
            }
            CheckedExpr::Interpolation { parts, .. } => {
                self.atom("interpolation");
                self.seq(parts, Self::interpolation_part);
            }
        }
    }

    fn interpolation_part(&mut self, part: &CheckedInterpolationPart) {
        match part {
            CheckedInterpolationPart::Text { text, .. } => {
                self.atom("interpolation_text");
                self.atom(text);
            }
            CheckedInterpolationPart::Expr(expr) => {
                self.atom("interpolation_expr");
                self.expr(expr);
            }
        }
    }

    fn arg(&mut self, arg: &CheckedArg) {
        self.atom("arg");
        self.option(arg.name.as_ref(), |this, name| this.atom(name));
        self.expr(&arg.value);
    }

    fn call_target(&mut self, target: &CheckedCallTarget) {
        match target {
            CheckedCallTarget::SavedIndexLookup => self.atom("target_saved_index_lookup"),
            CheckedCallTarget::SavedLayerRead => self.atom("target_saved_layer_read"),
            CheckedCallTarget::SavedResourceRead => self.atom("target_saved_resource_read"),
            CheckedCallTarget::IdentityConstructor(constructor) => {
                self.atom("target_identity_constructor");
                self.identity_constructor(constructor);
            }
            CheckedCallTarget::ErrorConstructor => self.atom("target_error_constructor"),
            CheckedCallTarget::Builtin(builtin) => {
                self.atom("target_builtin");
                self.builtin(*builtin);
            }
            CheckedCallTarget::Std(call) => {
                self.atom("target_std");
                self.std_call(call);
            }
            CheckedCallTarget::ResourceConstructor(constructor) => {
                self.atom("target_resource_constructor");
                self.resource_constructor(constructor);
            }
            CheckedCallTarget::LocalCollection { name } => {
                self.atom("target_local_collection");
                self.atom(name);
            }
            CheckedCallTarget::Function(target) => {
                self.atom("target_function");
                self.function_ref(*target);
            }
        }
    }

    fn identity_constructor(&mut self, constructor: &CheckedIdentityConstructor) {
        self.atom(&constructor.root);
        self.seq(&constructor.keys, Self::saved_key_param);
    }

    fn resource_constructor(&mut self, constructor: &CheckedResourceConstructor) {
        self.resource_ref(constructor.resource, &constructor.name);
        self.atom(&constructor.name);
        self.seq(&constructor.fields, Self::resource_constructor_field);
    }

    fn resource_constructor_field(&mut self, field: &CheckedResourceConstructorField) {
        self.atom(&field.name);
        self.bool(field.required);
        self.runtime_type(&field.ty);
    }

    fn resource_ref(&mut self, resource: CheckedResourceRef, name: &str) {
        self.atom("resource_ref");
        let resource = self
            .program
            .facts()
            .resources()
            .iter()
            .find(|fact| fact.module.0 == resource.module && fact.name == name);
        match resource.and_then(|fact| fact.catalog_id.as_deref()) {
            Some(catalog_id) => {
                self.atom("catalog");
                self.atom(catalog_id);
            }
            None => self.atom("unavailable"),
        }
    }

    fn function_ref(&mut self, target: CheckedFunctionRef) {
        match function_by_ref(self.program, target, SourceSpan::default()) {
            Ok((module, function)) => self.atom(&canonical_entry_name(module, function)),
            Err(_) => self.atom("unavailable"),
        }
        self.return_presence(target.presence);
    }

    fn std_call(&mut self, call: &CheckedStdCall) {
        self.atom(call.module);
        self.atom(call.op);
        self.return_presence(call.presence);
        self.option(call.requires_capability.as_ref(), |this, capability| {
            this.capability(*capability);
        });
    }

    fn saved_place(&mut self, place: &CheckedSavedPlace) {
        self.atom("place");
        self.catalog_or_name(place.store_catalog_id.as_deref(), &place.root);
        self.seq(&place.root_members, Self::saved_member);
        self.seq(&place.members, Self::saved_member);
        self.seq(&place.indexes, Self::saved_index);
        self.seq(&place.identity_keys, Self::saved_key_param);
        self.seq(&place.layers, Self::saved_layer);
        self.saved_terminal(&place.terminal);
    }

    fn saved_index(&mut self, index: &CheckedSavedIndex) {
        self.atom("saved_index");
        self.catalog_or_name(index.catalog_id.as_deref(), &index.name);
        self.bool(index.unique);
        self.seq(&index.keys, Self::saved_index_key);
    }

    fn saved_index_key(&mut self, key: &CheckedSavedIndexKey) {
        self.atom("saved_index_key");
        self.atom(&key.name);
        match key.source {
            StoreIndexKeySource::IdentityKey => self.atom("identity_key"),
            StoreIndexKeySource::ResourceMember(member) => {
                self.atom("resource_member");
                self.option(
                    self.program
                        .facts()
                        .resource_members()
                        .get(member.0 as usize),
                    |this, member| {
                        this.catalog_or_name(member.catalog_id.as_deref(), &member.name);
                    },
                );
            }
        }
        self.stored_value_meaning(&key.value_meaning);
    }

    fn saved_key_param(&mut self, key: &CheckedSavedKeyParam) {
        self.atom("saved_key_param");
        self.atom(&key.name);
        self.option(key.scalar.as_ref(), |this, scalar| this.scalar(*scalar));
    }

    fn saved_layer(&mut self, layer: &CheckedSavedLayer) {
        self.atom("saved_layer");
        self.catalog_or_name(layer.catalog_id.as_deref(), &layer.name);
        self.seq(&layer.args, Self::arg);
        self.seq(&layer.key_params, Self::saved_key_param);
        self.option(layer.leaf.as_ref(), Self::store_leaf);
        self.bool(layer.typed_entry);
        self.seq(&layer.members, Self::saved_member);
    }

    fn saved_member(&mut self, member: &CheckedSavedMember) {
        self.atom("saved_member");
        self.catalog_or_name(member.catalog_id.as_deref(), &member.name);
        self.seq(&member.key_params, Self::saved_key_param);
        match member.kind {
            CheckedSavedMemberKind::Field { required } => {
                self.atom("field");
                self.bool(required);
            }
            CheckedSavedMemberKind::Group => self.atom("group"),
        }
        self.option(member.leaf.as_ref(), Self::store_leaf);
        self.bool(member.typed_entry);
        self.seq(&member.group_members, Self::saved_member);
    }

    fn saved_terminal(&mut self, terminal: &CheckedSavedTerminal) {
        match terminal {
            CheckedSavedTerminal::Record => self.atom("terminal_record"),
            CheckedSavedTerminal::Field {
                name,
                catalog_id,
                leaf,
                ..
            } => {
                self.atom("terminal_field");
                self.catalog_or_name(catalog_id.as_deref(), name);
                self.option(leaf.as_ref(), Self::store_leaf);
            }
            CheckedSavedTerminal::Index {
                name,
                catalog_id,
                args,
                unique,
                arg_count,
                ..
            } => {
                self.atom("terminal_index");
                self.catalog_or_name(catalog_id.as_deref(), name);
                self.seq(args, Self::arg);
                self.bool(*unique);
                self.usize(*arg_count);
            }
        }
    }

    fn store_leaf(&mut self, leaf: &StoreLeafKind) {
        match leaf {
            StoreLeafKind::Scalar(scalar) => {
                self.atom("leaf_scalar");
                self.scalar(*scalar);
            }
            StoreLeafKind::Enum { enum_id } => {
                self.atom("leaf_enum");
                self.enum_id(*enum_id);
            }
            StoreLeafKind::Identity { store_root, arity } => {
                self.atom("leaf_identity");
                self.atom(store_root);
                self.usize(*arity);
            }
        }
    }

    fn stored_value_meaning(&mut self, meaning: &StoredValueMeaning) {
        match meaning {
            StoredValueMeaning::Scalar(scalar) => {
                self.atom("meaning_scalar");
                self.scalar(*scalar);
            }
            StoredValueMeaning::Identity {
                root,
                store_catalog_id,
                arity,
                key_scalars,
                ..
            } => {
                self.atom("meaning_identity");
                self.catalog_or_name(store_catalog_id.as_deref(), root);
                self.usize(*arity);
                self.seq(key_scalars, |this, scalar| this.scalar(*scalar));
            }
            StoredValueMeaning::Enum { enum_id, members } => {
                self.atom("meaning_enum");
                self.enum_id(*enum_id);
                self.seq(members, |this, member| this.enum_member_id(*member));
            }
        }
    }

    fn enum_member_ref(&mut self, member: &CheckedEnumMemberRef) {
        self.atom("enum_member_ref");
        self.enum_ref(&member.enum_ref);
        self.enum_member_id(member.member_id);
        self.seq(&member.member_uses, |this, (member_id, _span)| {
            this.enum_member_id(*member_id);
        });
    }

    fn enum_ref(&mut self, enum_ref: &CheckedEnumRef) {
        self.enum_id(enum_ref.enum_id);
    }

    fn enum_id(&mut self, enum_id: marrow_check::EnumId) {
        let Some(enum_fact) = self.program.facts().enum_(enum_id) else {
            self.atom("unavailable");
            return;
        };
        self.catalog_or_name(enum_fact.catalog_id.as_deref(), &enum_fact.name);
    }

    fn enum_member_id(&mut self, member_id: marrow_check::EnumMemberId) {
        let Some(member) = self.program.facts().enum_member(member_id) else {
            self.atom("unavailable");
            return;
        };
        self.catalog_or_name(member.catalog_id.as_deref(), &member.name);
    }

    fn catalog_or_name(&mut self, catalog_id: Option<&str>, name: &str) {
        match catalog_id {
            Some(catalog_id) => {
                self.atom("catalog");
                self.atom(catalog_id);
            }
            None => {
                self.atom("name");
                self.atom(name);
            }
        }
    }

    fn runtime_type(&mut self, ty: &CheckedRuntimeValueType) {
        match ty {
            CheckedRuntimeValueType::Primitive(scalar) => {
                self.atom("runtime_scalar");
                self.scalar(*scalar);
            }
            CheckedRuntimeValueType::Error => self.atom("runtime_error"),
            CheckedRuntimeValueType::Resource => self.atom("runtime_resource"),
            CheckedRuntimeValueType::GroupEntry => self.atom("runtime_group_entry"),
            CheckedRuntimeValueType::Identity { root, keys } => {
                self.atom("runtime_identity");
                self.atom(root);
                self.option(keys.as_ref(), |this, keys| {
                    this.seq(keys, |this, key| {
                        this.atom(&key.name);
                        this.schema_type(&key.ty);
                    });
                });
            }
            CheckedRuntimeValueType::Enum {
                module,
                name,
                enum_id,
                allowed_members,
            } => {
                self.atom("runtime_enum");
                self.atom(module);
                self.atom(name);
                let enum_fact = enum_id.and_then(|id| self.program.facts().enum_(id));
                self.option(enum_fact, |this, enum_fact| {
                    this.catalog_or_name(enum_fact.catalog_id.as_deref(), &enum_fact.name);
                });
                self.seq(allowed_members, |this, member| this.enum_member_id(*member));
            }
            CheckedRuntimeValueType::Sequence(element) => {
                self.atom("runtime_sequence");
                self.runtime_type(element);
            }
            CheckedRuntimeValueType::LocalTree { keys, value } => {
                self.atom("runtime_local_tree");
                self.seq(keys, Self::runtime_type);
                self.runtime_type(value);
            }
            CheckedRuntimeValueType::Invalid => self.atom("runtime_invalid"),
            CheckedRuntimeValueType::Unknown => self.atom("runtime_unknown"),
        }
    }

    fn marrow_type(&mut self, ty: &MarrowType) {
        match ty {
            MarrowType::Primitive(scalar) => {
                self.atom("type_scalar");
                self.scalar(*scalar);
            }
            MarrowType::Error => self.atom("type_error"),
            MarrowType::Resource(resource) => {
                self.atom("type_resource");
                self.atom(resource);
            }
            MarrowType::GroupEntry { resource, layers } => {
                self.atom("type_group_entry");
                self.atom(resource);
                self.seq(layers, |this, layer| this.atom(layer));
            }
            MarrowType::Identity(root) => {
                self.atom("type_identity");
                self.atom(root);
            }
            MarrowType::Enum { module, name } => {
                self.atom("type_enum");
                self.atom(module);
                self.atom(name);
            }
            MarrowType::Sequence(element) => {
                self.atom("type_sequence");
                self.marrow_type(element);
            }
            MarrowType::LocalTree { keys, value } => {
                self.atom("type_local_tree");
                self.seq(keys, Self::marrow_type);
                self.marrow_type(value);
            }
            MarrowType::Invalid => self.atom("type_invalid"),
            MarrowType::Unknown => self.atom("type_unknown"),
        }
    }

    fn schema_type(&mut self, ty: &Type) {
        match ty {
            Type::Scalar(scalar) => {
                self.atom("schema_scalar");
                self.scalar(*scalar);
            }
            Type::Sequence(element) => {
                self.atom("schema_sequence");
                self.schema_type(element);
            }
            Type::Identity(root) => {
                self.atom("schema_identity");
                self.atom(root);
            }
            Type::Named(name) => {
                self.atom("schema_named");
                self.atom(name);
            }
            Type::Unknown => self.atom("schema_unknown"),
        }
    }

    fn return_presence(&mut self, presence: ReturnPresence) {
        match presence {
            ReturnPresence::Always => self.atom("presence_always"),
            ReturnPresence::MaybePresent => self.atom("presence_maybe"),
        }
    }

    fn literal_kind(&mut self, kind: CheckedLiteralKind) {
        match kind {
            CheckedLiteralKind::Integer => self.atom("literal_integer"),
            CheckedLiteralKind::Decimal => self.atom("literal_decimal"),
            CheckedLiteralKind::Duration => self.atom("literal_duration"),
            CheckedLiteralKind::String => self.atom("literal_string"),
            CheckedLiteralKind::Bytes => self.atom("literal_bytes"),
            CheckedLiteralKind::Bool => self.atom("literal_bool"),
        }
    }

    fn unary_op(&mut self, op: CheckedUnaryOp) {
        match op {
            CheckedUnaryOp::Neg => self.atom("unary_neg"),
            CheckedUnaryOp::Not => self.atom("unary_not"),
        }
    }

    fn binary_op(&mut self, op: CheckedBinaryOp) {
        match op {
            CheckedBinaryOp::Multiply => self.atom("binary_multiply"),
            CheckedBinaryOp::Divide => self.atom("binary_divide"),
            CheckedBinaryOp::Remainder => self.atom("binary_remainder"),
            CheckedBinaryOp::Add => self.atom("binary_add"),
            CheckedBinaryOp::Subtract => self.atom("binary_subtract"),
            CheckedBinaryOp::RangeExclusive => self.atom("binary_range_exclusive"),
            CheckedBinaryOp::RangeInclusive => self.atom("binary_range_inclusive"),
            CheckedBinaryOp::Less => self.atom("binary_less"),
            CheckedBinaryOp::LessEqual => self.atom("binary_less_equal"),
            CheckedBinaryOp::Greater => self.atom("binary_greater"),
            CheckedBinaryOp::GreaterEqual => self.atom("binary_greater_equal"),
            CheckedBinaryOp::Equal => self.atom("binary_equal"),
            CheckedBinaryOp::NotEqual => self.atom("binary_not_equal"),
            CheckedBinaryOp::Coalesce => self.atom("binary_coalesce"),
            CheckedBinaryOp::Is => self.atom("binary_is"),
            CheckedBinaryOp::And => self.atom("binary_and"),
            CheckedBinaryOp::Or => self.atom("binary_or"),
        }
    }

    fn builtin(&mut self, builtin: CheckedBuiltinCall) {
        match builtin {
            CheckedBuiltinCall::Print => self.atom("builtin_print"),
            CheckedBuiltinCall::Exists => self.atom("builtin_exists"),
            CheckedBuiltinCall::NextId => self.atom("builtin_next_id"),
            CheckedBuiltinCall::Append => self.atom("builtin_append"),
            CheckedBuiltinCall::Bytes => self.atom("builtin_bytes"),
            CheckedBuiltinCall::ErrorCode => self.atom("builtin_error_code"),
            CheckedBuiltinCall::Conversion(scalar) => {
                self.atom("builtin_conversion");
                self.scalar(scalar);
            }
            CheckedBuiltinCall::Keys => self.atom("builtin_keys"),
            CheckedBuiltinCall::Count => self.atom("builtin_count"),
            CheckedBuiltinCall::Values => self.atom("builtin_values"),
            CheckedBuiltinCall::Entries => self.atom("builtin_entries"),
            CheckedBuiltinCall::Reversed => self.atom("builtin_reversed"),
            CheckedBuiltinCall::Next => self.atom("builtin_next"),
            CheckedBuiltinCall::Prev => self.atom("builtin_prev"),
        }
    }

    fn capability(&mut self, capability: Capability) {
        match capability {
            Capability::Clock => self.atom("capability_clock"),
            Capability::Context => self.atom("capability_context"),
            Capability::Environment => self.atom("capability_environment"),
            Capability::Log => self.atom("capability_log"),
            Capability::Filesystem => self.atom("capability_filesystem"),
        }
    }

    fn scalar(&mut self, scalar: ScalarType) {
        self.atom(scalar.name());
    }
}
