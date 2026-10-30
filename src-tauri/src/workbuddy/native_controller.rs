//! Internal persistent-target -> original native prepare/Admission/apply/recover.
//! A fixed macOS install factory supplies source/process admission; no IPC or
//! real client invocation. Original
//! native Admission pins are Unix-only; other platforms fail before journal writes.
use super::{
    native::{self, NativeContext},
    native_owner::{self, Disposition},
    native_source::{Catalog, Receipt, SelectionKey},
    native_transaction::{
        self as transaction, Admission, Failure, FileIo, JournalOwner, Outcome, Paths,
    },
};
use crate::secrets::storage::StorageIdentity;
use std::path::{Path, PathBuf};

#[cfg(any(test, target_os = "macos"))]
#[cfg_attr(not(test), allow(dead_code))]
// Internal consumer; verified production source factory remains separate.
mod process;

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
mod install;

#[cfg(any(
    test,
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
pub(super) mod switch_adapter;

mod sealed {
    pub trait Sealed {}
}
/// A reviewed local source/process adapter owns actual source, realm/key-context
/// and stopped verification, including physical source identity. Path strings or
/// stored metadata alone are not proof.
/// Implementations are sealed here; the install factory retains physical pins
/// and composes the existing process admission. Fixture ports remain test-only.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) trait ControlledSource: sealed::Sealed {
    fn root(&self) -> &Path;
    fn context(&self) -> &NativeContext;
    /// Start a new operation's build-content admission, never a checkpoint refresh.
    fn begin_operation(&mut self) -> Result<(), Failure> {
        self.verify_stopped()
    }
    fn verify_stopped(&mut self) -> Result<(), Failure>;
    fn verify_pins(&mut self) -> Result<(), Failure> {
        self.verify_stopped()
    }
    fn invalidate(&mut self) {}
    /// Recheck the retained source revision/pins after process observation.
    /// No bulk reads here: expensive source work must precede that observation.
    fn verify_unchanged(&mut self) -> Result<(), Failure>;
}
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Controller<'a> {
    pub catalog: Catalog<'a>,
}
/// Data selection only, not a source proof or execution ticket.
pub(super) struct Selection {
    receipt: Receipt,
}
#[cfg(any(
    test,
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
fn selected_realm(selection: &Selection) -> Result<super::model::Domain, Failure> {
    super::model::Domain::parse(&selection.receipt.realm).map_err(|_| Failure::RequestConflict)
}
/// No Clone/Debug/serde: execution retains the original sealed Admission pin.
pub(super) struct Plan {
    receipt: Receipt,
    storage: StorageIdentity,
    epoch: u64,
    selector: String,
    root: PathBuf,
    context: NativeContext,
    ticket: Admission,
}
/// Concrete backend handoff, not yet an IPC/UI action. Dropping it only releases
/// pins; an already-persisted pending journal remains the original owner's work.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(dead_code)]
pub(super) struct InstalledPlan {
    plan: Plan,
    source: install::Source,
}
/// Private-construction access witness. Stored data cannot directly mint a native
/// snapshot; only the controller creates this after a sealed source verification.
pub(super) struct ReadPermit {
    context: NativeContext,
}
impl ReadPermit {
    pub(super) fn allows(&self, context: &NativeContext) -> bool {
        self.context == *context
    }
}
fn verify_bound(
    source: &mut impl ControlledSource,
    root: &Path,
    context: &NativeContext,
) -> Result<(), Failure> {
    source.verify_stopped()?;
    if source.root() != root || source.context() != context {
        return Err(Failure::SourceChanged);
    }
    Ok(())
}
fn scope(source: &mut impl ControlledSource) -> Result<(PathBuf, NativeContext, Paths), Failure> {
    source.begin_operation()?;
    let root = source.root().to_path_buf();
    let context = source.context().clone();
    let paths = Paths::at(&root)?;
    Ok((root, context, paths))
}
#[cfg_attr(not(test), allow(dead_code))]
impl Controller<'_> {
    #[cfg(all(feature = "gui", target_os = "macos", target_arch = "aarch64"))]
    pub(super) async fn activate_with_restart(
        &self,
        key: &SelectionKey,
        bundle: &Path,
    ) -> Result<super::native_switch_lifecycle::Report, super::native_switch_lifecycle::Error> {
        use super::native_switch_lifecycle::{self as lifecycle, Error};
        let selected = self.select(key).await.map_err(Error::Transaction)?;
        let realm = selected_realm(&selected).map_err(Error::Transaction)?;
        let (mut driver, source, witness) =
            switch_adapter::mac::create(bundle, realm).map_err(Error::Transaction)?;
        // The original shared Source retains its physical pins. Only its
        // synchronous admission methods need a RefCell borrow, not the waits
        // for catalog/journal ownership inside the asynchronous transaction.
        struct TransactionSource<'a> {
            shared: &'a std::cell::RefCell<install::Source>,
            root: PathBuf,
            context: NativeContext,
        }
        impl TransactionSource<'_> {
            fn with_source(
                &self,
                apply: impl FnOnce(&mut install::Source) -> Result<(), Failure>,
            ) -> Result<(), Failure> {
                let mut source = self.shared.borrow_mut();
                if source.root() != self.root || source.context() != &self.context {
                    return Err(Failure::SourceChanged);
                }
                let result = apply(&mut source);
                if source.root() != self.root || source.context() != &self.context {
                    return Err(Failure::SourceChanged);
                }
                result
            }
        }
        impl sealed::Sealed for TransactionSource<'_> {}
        impl ControlledSource for TransactionSource<'_> {
            fn root(&self) -> &Path {
                &self.root
            }
            fn context(&self) -> &NativeContext {
                &self.context
            }
            fn begin_operation(&mut self) -> Result<(), Failure> {
                self.with_source(ControlledSource::begin_operation)
            }
            fn verify_stopped(&mut self) -> Result<(), Failure> {
                self.with_source(ControlledSource::verify_stopped)
            }
            fn verify_pins(&mut self) -> Result<(), Failure> {
                self.with_source(ControlledSource::verify_pins)
            }
            fn invalidate(&mut self) {
                self.shared.borrow_mut().invalidate();
            }
            fn verify_unchanged(&mut self) -> Result<(), Failure> {
                self.with_source(ControlledSource::verify_unchanged)
            }
        }
        let (root, context) = {
            let source = source.borrow();
            (source.root().to_owned(), source.context().clone())
        };
        let mut transaction_source = TransactionSource {
            shared: &source,
            root,
            context,
        };
        lifecycle::run(&mut driver, || async {
            let preimage = witness.borrow_mut().take().ok_or(Failure::SourceChanged)?;
            self.activate_guarded(
                key,
                &mut transaction_source,
                &mut transaction::Files,
                &preimage,
            )
            .await
        })
        .await
    }

    #[cfg(any(
        test,
        all(feature = "gui", target_os = "macos", target_arch = "aarch64")
    ))]
    pub(super) async fn activate_guarded(
        &self,
        key: &SelectionKey,
        source: &mut impl ControlledSource,
        io: &mut dyn FileIo,
        preimage: &super::native_source::Pair,
    ) -> Result<Outcome, Failure> {
        let unchanged = |source: &dyn ControlledSource| {
            super::native_source::read_pair(&source.root().join("workbuddy-desktop.info"))
                .map_err(|_| Failure::SourceChanged)
                .and_then(|pair| {
                    if pair == *preimage {
                        Ok(())
                    } else {
                        Err(Failure::SourceChanged)
                    }
                })
        };
        unchanged(source)?;
        let selected = self.select(key).await?;
        let plan = self.prepare(&selected, source, io).await?;
        unchanged(source)?;
        self.apply(&plan, source, io).await
    }

    pub(super) async fn activate(
        &self,
        key: &SelectionKey,
        source: &mut impl ControlledSource,
        io: &mut dyn FileIo,
    ) -> Result<Outcome, Failure> {
        let target = self.select(key).await?;
        let plan = self.prepare(&target, source, io).await?;
        self.apply(&plan, source, io).await
    }

    pub(super) async fn journal_binding(
        &self,
    ) -> Result<(super::model::Domain, transaction::Stamp), Failure> {
        native_owner::with_selected_owner(
            self.catalog.session,
            self.catalog.identity,
            self.catalog.epoch,
            self.catalog.selection,
            |owner| {
                if owner.disposition()? == Disposition::Absent {
                    return Err(Failure::RecoveryRequired);
                }
                let (stamp, journal) = owner.load()?;
                let journal = journal.ok_or(Failure::RecoveryRequired)?;
                let realm = super::model::Domain::parse(journal.realm())
                    .map_err(|_| Failure::RecoveryRequired)?;
                Ok((realm, stamp))
            },
        )
        .await
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub(super) async fn activate_installed(
        &self,
        key: &SelectionKey,
        bundle: &Path,
    ) -> Result<Outcome, Failure> {
        let selected = self.select(key).await?;
        let mut plan = self.prepare_installed(&selected, bundle).await?;
        self.apply_installed(&mut plan).await
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub(super) async fn capture_installed(
        &self,
        bundle: &Path,
        expected: Option<&SelectionKey>,
        label: &str,
    ) -> Result<Receipt, Failure> {
        let mut source = install::open_capture(bundle)?;
        self.capture(&mut source, expected, label).await
    }
    pub(super) async fn capture(
        &self,
        source: &mut impl ControlledSource,
        expected: Option<&SelectionKey>,
        label: &str,
    ) -> Result<Receipt, Failure> {
        let (root, context, _) = scope(source)?;
        let observation = super::native_source::Observation::read(
            &root.join("workbuddy-desktop.info"),
            &context,
        )?;
        let expected = match expected {
            Some(key) => Some(self.catalog.select(key).await?),
            None => None,
        };
        self.catalog
            .save_checked(&observation, expected.as_ref(), label, || {
                verify_bound(source, &root, &context)
            })
            .await
    }
    /// Production source construction is fixed to the reviewed public build;
    /// neither caller-supplied context strings nor fixture pins are accepted.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[allow(dead_code)]
    pub(super) async fn prepare_installed(
        &self,
        target: &Selection,
        bundle: &Path,
    ) -> Result<InstalledPlan, Failure> {
        let realm = super::model::Domain::parse(&target.receipt.realm)
            .map_err(|_| Failure::RequestConflict)?;
        let mut source = install::open(bundle, realm)?;
        let plan = self
            .prepare(target, &mut source, &mut transaction::Files)
            .await?;
        Ok(InstalledPlan { plan, source })
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[allow(dead_code)]
    pub(super) async fn apply_installed(
        &self,
        prepared: &mut InstalledPlan,
    ) -> Result<Outcome, Failure> {
        self.apply(
            &prepared.plan,
            &mut prepared.source,
            &mut transaction::Files,
        )
        .await
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub(super) async fn recover_installed_bound(
        &self,
        bundle: &Path,
        realm: super::model::Domain,
        expected: &transaction::Stamp,
    ) -> Result<Outcome, Failure> {
        let mut source = install::open(bundle, realm)?;
        self.recover_bound(&mut source, &mut transaction::Files, Some(expected))
            .await
    }
    pub(super) async fn select(&self, key: &SelectionKey) -> Result<Selection, Failure> {
        Ok(Selection {
            receipt: self.catalog.select(key).await?,
        })
    }
    pub(super) async fn prepare(
        &self,
        target: &Selection,
        source: &mut impl ControlledSource,
        io: &mut dyn FileIo,
    ) -> Result<Plan, Failure> {
        if !cfg!(unix) {
            return Err(Failure::UnsafePath);
        }
        let (root, context, paths) = scope(source)?;
        native_owner::with_selected_owner(
            self.catalog.session,
            self.catalog.identity,
            self.catalog.epoch,
            self.catalog.selection,
            |owner| {
                verify_bound(source, &root, &context)?;
                let row = target
                    .receipt
                    .read_under_owner(owner, self.catalog.identity.store_id())?;
                let permit = ReadPermit {
                    context: context.clone(),
                };
                let selected = row
                    .account
                    .native_for_controller(&permit)
                    .map_err(|_| Failure::RequestConflict)?;
                let before = transaction::observe(io, &paths)?;
                let current =
                    native::capture(before.auth_bytes().ok_or(Failure::SourceChanged)?, &context)
                        .map_err(|_| Failure::SourceChanged)?;
                let prepared =
                    native::prepare(&current, &selected).map_err(|_| Failure::RequestConflict)?;
                verify_bound(source, &root, &context)?;
                // Pin the actual original native root before creating its registered owner.
                // Admission cannot be reconstructed from stored bytes at apply time.
                // Existing initialization/CAS are reused, never a second journal.
                let disposition = owner.disposition()?;
                if disposition == Disposition::Pending {
                    return Err(Failure::RecoveryRequired);
                }
                // Validate platform/root pin without writing the journal first.
                let root_pin = transaction::validate_admission_root(&paths)?;
                if disposition == Disposition::Absent {
                    owner.initialize()?;
                }
                let (stamp, _) = owner.load()?;
                let request = uuid::Uuid::new_v4().to_string();
                let ticket =
                    Admission::preview(&paths, &context, &stamp, &request, &before, &prepared)?;
                drop(root_pin);
                verify_bound(source, &root, &context)?;
                Ok(Plan {
                    receipt: target.receipt.clone(),
                    storage: self.catalog.identity.clone(),
                    epoch: self.catalog.epoch,
                    selector: self.catalog.selection.selection_digest().into(),
                    root,
                    context,
                    ticket,
                })
            },
        )
        .await
    }
    pub(super) async fn apply(
        &self,
        plan: &Plan,
        source: &mut impl ControlledSource,
        io: &mut dyn FileIo,
    ) -> Result<Outcome, Failure> {
        if !cfg!(unix) {
            return Err(Failure::UnsafePath);
        }
        if *self.catalog.identity != plan.storage
            || self.catalog.epoch != plan.epoch
            || self.catalog.selection.selection_digest() != plan.selector
        {
            return Err(Failure::SourceChanged);
        }
        source.begin_operation()?;
        if source.root() != plan.root || source.context() != &plan.context {
            return Err(Failure::SourceChanged);
        }
        let paths = Paths::at(&plan.root)?;
        native_owner::with_selected_owner(
            self.catalog.session,
            self.catalog.identity,
            self.catalog.epoch,
            self.catalog.selection,
            |owner| {
                verify_bound(source, &plan.root, &plan.context)?;
                plan.receipt
                    .read_under_owner(owner, self.catalog.identity.store_id())?;
                let mut stopped = || verify_bound(source, &plan.root, &plan.context).is_ok();
                transaction::apply(&paths, io, owner, &mut stopped, &plan.context, &plan.ticket)
            },
        )
        .await
    }
    pub(super) async fn recover(
        &self,
        source: &mut impl ControlledSource,
        io: &mut dyn FileIo,
    ) -> Result<Outcome, Failure> {
        self.recover_bound(source, io, None).await
    }
    pub(super) async fn recover_bound(
        &self,
        source: &mut impl ControlledSource,
        io: &mut dyn FileIo,
        expected: Option<&transaction::Stamp>,
    ) -> Result<Outcome, Failure> {
        if !cfg!(unix) {
            return Err(Failure::UnsafePath);
        }
        let (root, context, paths) = scope(source)?;
        native_owner::with_selected_owner(
            self.catalog.session,
            self.catalog.identity,
            self.catalog.epoch,
            self.catalog.selection,
            |owner| {
                verify_bound(source, &root, &context)?;
                // Recovery consumes its persisted original Stamp, not a fabricated
                // refreshed account selection or a newly initialized journal.
                let (stamp, _) = owner.load()?;
                if expected.is_some_and(|original| *original != stamp) {
                    return Err(Failure::RecoveryRequired);
                }
                let mut stopped = || verify_bound(source, &root, &context).is_ok();
                transaction::recover(&paths, io, owner, &mut stopped, &stamp, &context)
            },
        )
        .await
    }
}
#[cfg(all(test, unix))]
#[path = "native_controller_tests.rs"]
mod tests;

/// Opaque one-use attachment plan: original row FD plus reviewed observation.
/// This is neither native activation admission nor an account/journal owner.
#[cfg(any(
    all(test, unix),
    all(target_os = "macos", target_arch = "aarch64", feature = "gui")
))]
pub(super) struct AttachPlan {
    capability: super::native_source::ApiAttachCapability,
    observation: super::native_source::Observation,
    root: PathBuf,
    context: NativeContext,
}
/// The concrete factory's held pins survive the UI review; callers cannot forge
/// a source from a renderer-provided path, realm, or stored credential.
#[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
pub(super) struct InstalledAttachPlan {
    plan: AttachPlan,
    source: install::Source,
}
#[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
impl InstalledAttachPlan {
    pub(super) fn review(&self) -> (&str, &str, &str, std::time::Instant) {
        self.plan.capability.review()
    }
}
#[cfg(any(
    all(test, unix),
    all(target_os = "macos", target_arch = "aarch64", feature = "gui")
))]
impl Controller<'_> {
    pub(super) async fn prepare_attach(
        &self,
        id: &str,
        source: &mut impl ControlledSource,
    ) -> Result<AttachPlan, Failure> {
        let (root, context, _) = scope(source)?;
        let observation = super::native_source::Observation::read(
            &root.join("workbuddy-desktop.info"),
            &context,
        )?;
        let capability = self.catalog.prepare_api_attach(id).await?;
        // Recheck the original pins and observed pair after waiting for the owner.
        verify_bound(source, &root, &context)?;
        observation.verify()?;
        Ok(AttachPlan {
            capability,
            observation,
            root,
            context,
        })
    }
    pub(super) async fn confirm_attach(
        &self,
        plan: AttachPlan,
        source: &mut impl ControlledSource,
    ) -> Result<Receipt, Failure> {
        // Do not refresh the retained installation revision after user review.
        // The original pins must still hold, including after the owner's wait
        // and immediately before account publication, not merely at preflight.
        verify_bound(source, &plan.root, &plan.context)?;
        self.catalog
            .confirm_api_attach_checked(plan.capability, &plan.observation, || {
                verify_bound(source, &plan.root, &plan.context)
            })
            .await
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub(super) async fn prepare_attach_installed(
        &self,
        id: &str,
        bundle: &Path,
    ) -> Result<InstalledAttachPlan, Failure> {
        let mut source = install::open_capture(bundle)?;
        let plan = self.prepare_attach(id, &mut source).await?;
        Ok(InstalledAttachPlan { plan, source })
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub(super) async fn confirm_attach_installed(
        &self,
        prepared: InstalledAttachPlan,
    ) -> Result<Receipt, Failure> {
        let InstalledAttachPlan { plan, mut source } = prepared;
        self.confirm_attach(plan, &mut source).await
    }
}
