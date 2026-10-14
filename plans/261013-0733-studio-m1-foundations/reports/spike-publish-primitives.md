# Spike S3: no-clobber publish on Windows and macOS

Date: 2026-10-14. Hosts: this Windows Server 2022 VM (NTFS, Administrator, Rust 1.98.1 MSVC), and
hosted `macos-15` (macOS 15.7.9, APFS, arm64) and `windows-2022` runners on GitHub Actions.

## Result

**Pass.** Both prototypes give every guarantee the Linux module gives, and both pass all five
test kinds: Windows here and on CI, macOS on CI. `FileRenameInformationEx` with `RootDirectory` set
and no replace flag works on NTFS, so the Windows design needs no path-based call for a live name.
The prototype also turned up four Windows differences that M3 phase 1 has to carry:

- Windows renames the opened file, not the name.
- NTFS will not rename an ancestor of a held handle.
- Cloud placeholders are hidden from the process unless it opts in.
- SMB rejects the handle-relative rename.

| Exit criterion | Status |
|---|---|
| Windows prototype passes all five test kinds | Done: 12 integration tests, 1 cloud-placeholder test and 2 unit tests pass on this VM and on `windows-2022` |
| macOS prototype passes all five test kinds | Done: 10 integration tests and 2 unit tests pass on `macos-15` |
| Cloud placeholder decision | Refuse mutation; tested against a real Cloud Files placeholder (below) |
| Report with an M3 phase 1 re-estimate | This report |

- Branch: `spike/m1-publish` (throwaway). The prototype was last tested at commit `d55ccc4`.
- Crate: `desktop/spikes/publish-primitives/`, its own `[workspace]`.
- CI run (macOS and Windows both green): https://github.com/khanhthanhdev/fframes-desktop/actions/runs/38489728612
- The workflow is the temporary `.github/workflows/spike-publish.yml`, present only on the spike branch.
- Each push also started `desktop-self-hosted.yml`, which has no branch filter. I cancelled those runs because they are not part of the spike.

## Guarantees of the Linux module (`studio-project/src/publish.rs`)

1. **Operations relative to a held handle.** `Dir` holds a directory descriptor. Every name is
   one validated component resolved with `*at` calls, so a path swap above the handle cannot
   redirect an operation.
2. **No link following.** Every open uses `O_NOFOLLOW` and stats use `AT_SYMLINK_NOFOLLOW`.
   `open_regular` also refuses non-regular files such as FIFOs and directories. `create_new`
   uses `O_EXCL`, so it refuses any existing name, including a dangling link.
3. **Atomic no-replace rename.** Moving a name an editor controls uses
   `renameat2(RENAME_NOREPLACE)` only. An existing destination returns `AlreadyExists` and leaves
   both names untouched. `probe_no_clobber` proves the primitive on the project's filesystem and
   blocks Apply when it fails. The link-then-unlink fallback is never used for live names.
4. **Root pathname binding.** `open_root` walks the canonical path from `/` without following
   links and records the `(dev, ino)` of every directory. `check_root_binding` repeats the walk,
   so it catches a root that was renamed away, replaced by a link, or reached through a swapped
   ancestor.
5. **Detection of a replaced source.** The rename moves whatever is at the name. The transaction
   layer then re-verifies the identity and hash of the moved file (and `is_still_at` the identity
   of each directory), so an editor's replacement is detected and kept.

## Windows design (`src/windows.rs`, `windows-sys` 0.61.2)

- **Every open** goes through `NtCreateFile`:
  - `OBJECT_ATTRIBUTES.RootDirectory` is the held handle and `ObjectName` is a single component.
    Only the volume root is opened with a null `RootDirectory`, as `\??\X:\`.
  - Attributes are `OBJ_CASE_INSENSITIVE`, which matches Win32 name semantics; M3 phase 3 owns
    case aliases.
  - Options are `FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT`, with `SYNCHRONIZE`
    added to every access mask.
  - Directories add `FILE_DIRECTORY_FILE`; regular files add `FILE_NON_DIRECTORY_FILE`.
  - Share mode is `READ | WRITE | DELETE`, except on the rename source (see below). Without
    `FILE_SHARE_DELETE` on directory handles, the user could not rename the project folder
    while the app holds it.
- **No-follow is a check, not a flag.** `FILE_OPEN_REPARSE_POINT` opens the reparse point
  itself: a junction opened this way looks like an empty directory. So every open is followed by
  `NtQueryInformationFile(FileAttributeTagInformation)`, and anything with
  `FILE_ATTRIBUTE_REPARSE_POINT` is classified by its tag and refused:
  - `IO_REPARSE_TAG_SYMLINK` and `IO_REPARSE_TAG_MOUNT_POINT` (junction) are links.
  - `IO_REPARSE_TAG_CLOUD*` is a cloud placeholder.
  - Any other tag is "other". This includes dedup (`0x80000013`), WOF and AppExecLink.
  - `FILE_ATTRIBUTE_RECALL_ON_OPEN` and `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` without a reparse
    attribute are also treated as cloud placeholders.
  - The tests assert that the refusal comes from this check: `open_child("junction")` fails with
    "is a link".
- **Identity.** `FileIdInformation` returns `VolumeSerialNumber` (u64) and the 128-bit
  `FILE_ID_128` in one call. They become `FileId { volume: u64, file: u128 }`. Example from CI:
  `8c64df9264df7e00:…0003000000142a7f`. `FileStandardInformation` gives size, link count and the
  directory flag. `FileBasicInformation` gives `LastWriteTime` and `ChangeTime`, the analogs of
  `st_mtim` and `st_ctim` for the "changed while read" check.
- **Rename:**
  1. Open the source relative to its directory with `DELETE | FILE_READ_ATTRIBUTES` and share
     `READ | WRITE`, deliberately without `FILE_SHARE_DELETE`.
  2. Stat that handle. Refuse a placeholder or an "other" reparse point, and, in the checked
     variant, any file id other than the one the caller verified.
  3. Call `NtSetInformationFile(handle, FILE_RENAME_INFORMATION { Flags: 0, RootDirectory:
     target_dir_handle, FileNameLength, FileName: to }, FileRenameInformationEx)`. The buffer is
     8-byte aligned and has no `FILE_RENAME_REPLACE_IF_EXISTS`.

  An existing destination returns `STATUS_OBJECT_NAME_COLLISION`, which maps through
  `RtlNtStatusToDosError` to `ERROR_ALREADY_EXISTS` and then `ErrorKind::AlreadyExists`. This
  holds within one directory and across directories, and when the destination is a file, a
  symlink, a dangling symlink, a directory symlink or a junction.
- **Create, mkdir, unlink:**
  - `create_new` uses `FILE_CREATE`. NTFS reports `FILE_CREATE` onto an existing directory link
    as access denied rather than a collision, so a failed create onto a name that exists is
    returned as `AlreadyExists`.
  - `mkdir` uses `FILE_CREATE | FILE_DIRECTORY_FILE`.
  - `unlink` opens with `DELETE` and sets
    `FileDispositionInformationEx(FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS)`.
    It removes a link as itself.
- **Root binding.** `open_root` canonicalizes once to `\\?\X:\…`, opens `\??\X:\`, and walks
  each component relative to the previous handle with the reparse check, recording each `FileId`.
  The prototype rejects UNC paths with `Unsupported`.
- **Component validation** rejects `\ / : * ? " < > |`, control characters, `.` and `..`, and
  any trailing dot or space. Rejecting `:` blocks alternate data streams: `plain:stream` is
  refused in a test. The trailing dot or space matters because NT would create a name that Win32
  tools cannot open.
- No path-based Win32 call (`MoveFileExW`, `CreateFileW`, `DeleteFileW`) touches a live name. The
  tests themselves use `std::fs` to play the editor.

### Windows differences M3 must carry

1. **Windows renames the opened file; Linux and macOS rename the name.** The NT sequence is
   open-then-rename, so it has a window. The source handle excludes `FILE_SHARE_DELETE`, so
   while the window is open nobody can rename, replace or delete the source name.
   - **Test result:** an editor's rename-over inside the window, through both `std::fs::rename` and a direct `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`, failed with `ERROR_ACCESS_DENIED`
     (5), here and on CI. The editor's bytes stayed in its temporary file, our move took exactly
     the verified file, and the editor's retry afterwards landed.
   - **Replaced source:** the identity check runs on the very handle that is renamed. A source
     replaced before the call is therefore refused before anything moves, which is stronger than
     the Linux detect-afterwards.
   - **Cost:** the open fails with a sharing violation while another process holds the file
     without `FILE_SHARE_DELETE`, which most Win32 programs do while a file is open. The bounded
     `ERROR_SHARING_VIOLATION` retry already listed in M3's risks covers this. An editor that
     saves during those microseconds sees one failed save.
   - The alternative, sharing `DELETE` on the source, was not tested. With it, an editor's
     rename-over would orphan the opened file, and a rename of the source away would be followed
     by our handle.
2. **NTFS refuses to rename a directory while any handle below it is open.** Renaming the
   project root itself works, and the binding catches it with symlink, junction and look-alike
   directory replacements. Renaming an ancestor of a held root fails with `ERROR_ACCESS_DENIED`,
   so the Linux "swapped ancestor" attack cannot happen on Windows while the root is held. The
   Windows test asserts that refusal and the macOS test keeps the Linux form.
   - **UX consequence:** while the app holds a project open, the user cannot rename or move the
     folders that contain it.
3. **Cloud placeholders are disguised by default** (see the decision below). The process must
   call `RtlSetProcessPlaceholderCompatibilityMode(PHCM_EXPOSE_PLACEHOLDERS)`, which
   `Dir::open_root` does. This setting is process-wide.
4. **SMB has no handle-relative rename.** See "Filesystems without the rename".

## macOS design (`src/macos.rs`, `libc` 0.2.190)

The Linux module carries over almost line for line:

- Opens use `openat(dirfd, name, … | O_NOFOLLOW | O_CLOEXEC)`, with `O_DIRECTORY` for
  directories and `O_NONBLOCK` for regular files so a FIFO cannot block.
- Stats use `fstatat(…, AT_SYMLINK_NOFOLLOW)`.
- `create_new` uses `O_CREAT | O_EXCL`; `mkdirat` and `unlinkat` work as on Linux.
- Rename uses `renameatx_np(fromfd, from, tofd, to, RENAME_EXCL)`, which returns `EEXIST`
  (`AlreadyExists`) and has no window.
- The root walk starts from `/`. `canonicalize` resolves `/var` to `/private/var` first, and the
  walk then uses `openat(O_NOFOLLOW | O_DIRECTORY)`, which returns `ENOTDIR` on a link.
- **Replaced source:** the rename is by name, as on Linux, so `rename_noreplace_checked` compares
  the moved file's identity with the verified one afterwards. On a mismatch it moves the file
  back with `RENAME_EXCL`. If the name was taken again in the meantime, the file stays at the
  destination and the error says so. The editor's bytes are never lost or overwritten.

## Tests

`tests/publish.rs` (the five kinds), `tests/cloud_placeholder.rs` (Windows), and unit tests in
`src/lib.rs`:

| Kind | Test | Windows | macOS |
|---|---|---|---|
| No-replace | `the_probe_proves_no_clobber_publication_on_this_filesystem_and_cleans_up` | pass | pass |
| No-replace | `a_no_replace_rename_refuses_an_existing_destination_and_moves_onto_a_free_name` (same and other directory) | pass | pass |
| Editor-save race | `the_rename_moves_the_original_and_an_editor_save_after_it_survives` (asserts the window exists on Windows and not on macOS) | pass | pass |
| Editor-save race | `an_editor_save_inside_the_open_rename_window_fails_without_losing_bytes` | pass | n/a (no window) |
| Editor-save race | `a_source_replaced_after_verification_is_detected_and_never_lost` | pass (refused before the move) | pass (moved back) |
| Links | `names_never_follow_links_or_open_special_files` (directory and file symlinks, dangling symlink, junction on Windows, FIFO on macOS, ADS name) | pass | pass |
| Swapped directory | `a_swapped_directory_is_detected_by_identity` (replacement directory, symlink, junction on Windows) | pass | pass |
| Root binding | `a_root_renamed_away_and_replaced_by_a_symlink_fails_the_pathname_binding` | pass | pass |
| Root binding | `a_root_renamed_away_and_replaced_by_a_junction_fails_the_pathname_binding` | pass | n/a |
| Root binding | `a_root_renamed_away_and_replaced_by_a_look_alike_directory_fails_the_pathname_binding` | pass | pass |
| Root binding | `a_swapped_ancestor_of_the_root_fails_the_pathname_binding` | n/a | pass |
| Root binding | `an_ancestor_of_a_held_root_cannot_be_swapped` | pass | n/a |
| Root binding | `a_root_reached_through_a_link_is_bound_to_its_real_path` | pass | pass |
| Cloud | `a_cloud_placeholder_is_refused_and_never_opened_or_moved` | pass | n/a |
| Cloud | unit `cloud_tags_are_classified_as_placeholders_and_nothing_else_is` | pass | pass |
| Filesystem | `the_probe_over_an_smb_share` (`#[ignore]`, run by hand here) | pass (probe refuses) | n/a |

- Creating symlinks needed no setup on this VM, which runs as Administrator, or on the hosted
  Windows runner. If symlink creation fails, the test helpers fail with a message naming
  `SeCreateSymbolicLinkPrivilege` rather than skipping.
- Junctions come from `mklink /J`.
- `cargo fmt --check` and `cargo clippy --all-targets -D warnings` are clean on both platforms.

## Cloud placeholders: decision

**Refuse mutation, with a reason that names the folder as cloud-synced and asks the user to move
the project to a local folder.** The sync provider can replace or dehydrate files under the
transaction, and an open of a placeholder can block on a network hydration.

- **Evidence.** The test registers a real sync root with `CfRegisterSyncRoot` (`cldflt` runs on
  this VM and on the hosted runner) and creates a placeholder with `CfCreatePlaceholders`.
  - The sync root directory itself carries `IO_REPARSE_TAG_CLOUD_3` (`0x9000301A`) and the file
    carries `IO_REPARSE_TAG_CLOUD_4` (`0x9000401A`).
  - `open_root` and `open_child` refuse the sync root. Inside it, `stat` reports
    `CloudPlaceholder`, `open_regular` refuses the file, and a rename from or onto it is refused.
  - The file is neither hydrated nor moved.
  - The test unregisters the sync root with `CfUnregisterSyncRoot` when it finishes. After the runs here, `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager` had no entries left.
- **The opt-in is required.** Before the process opts in, `std::fs::symlink_metadata` returned
  attributes `0x30` for the sync root and `0x400020` for the placeholder, with no
  `FILE_ATTRIBUTE_REPARSE_POINT`. Without `PHCM_EXPOSE_PLACEHOLDERS`, a cloud-synced project
  folder looks like an ordinary directory. The `RECALL_ON_DATA_ACCESS` fallback would still catch
  dehydrated files, but not the directories or hydrated files.
- **Consequence.** Because the sync root and its directories are cloud reparse points, a project
  anywhere inside a OneDrive folder is refused as a whole, at `open_root`. This matters on
  machines where OneDrive's Known Folder Move puts Documents and Desktop under OneDrive. The
  first-run project location should therefore default outside it.
- **Other reparse points.** The blanket refusal also refuses dedup files on Windows Server
  volumes with Data Deduplication and WOF-compressed files. I accept that for M3; a later
  allow-list of non-redirecting tags (dedup, WOF) is possible.
- The cloud-tag classification (`tag & !0xF000 == 0x9000001A`) is unit-tested across `CLOUD` and
  `CLOUD_1` to `CLOUD_F`, with negatives for mount point, symlink, dedup, WOF, AppExecLink,
  AF_UNIX and two near misses.

## Filesystems without the rename

The probe (`probe_no_clobber`, the same steps as Linux) is the gate. Wherever it fails, mutation
stays blocked.

- **SMB (tested).** I ran the probe through the existing `\\localhost\C$` administrative share,
  which changed no configuration. `FileRenameInformationEx` with a `RootDirectory` returned
  `ERROR_INVALID_PARAMETER` (87), and across directories onto an existing name it returned
  `ERROR_BAD_LENGTH` (24). In each case nothing moved and the probe refused. Network shares are
  refused.
- **FAT and exFAT (untested).** There is no FAT volume here, and creating one (a VHD or a USB
  stick) would change the machine, so I did not. Whether FAT supports `FileIdInformation` and
  `FileRenameInformationEx` is unknown. The probe refuses it either way, and `stat_handle` fails
  closed if `FileIdInformation` is unsupported.
- **ReFS (untested).** Expected to work, since it supports both information classes; the probe
  decides.
- **macOS:** APFS passes on CI. HFS+ is untested (whether a volume supports it shows in `VOL_CAP_INT_RENAME_EXCL`). SMB, NFS
  and FAT volumes may not, and the probe decides there too.

## Re-estimate of M3 phase 1 (Windows publication and executor)

The planned 5-7 days holds, and **5-6 days is now likely**. The prototype retires the main risk.

| Phase 1 task | Prototype covers | Remaining estimate |
|---|---|---|
| 4. NT handle-relative publication | Open, stat, create, mkdir, unlink, rename, probe and root binding exist and pass. Still missing: `rmdir`, a Windows `hash_regular` (compare `LastWriteTime`/`ChangeTime`/size/id around the read), directory durability (`NtFlushBuffersFile` on a directory handle needs write access; decide whether NTFS journaling is enough), and a bounded sharing-violation retry | 1-1.5d (was the bulk of the risk) |
| 3. `FileId` widening and journal bump | The shape is settled: `{ volume: u64, file: u128 }` (u128 for the macOS `st_ino`) | 1d |
| 5. Hard-link refusal | `nlink` is already collected on both platforms | 0.25d |
| 6. Cloud placeholder refusal | Classification, refusal message, process opt-in and a real placeholder test are done | 0.25d (port the test) |
| 7. Mode check | Not prototyped. Proposal: map the executable bit to "not applicable" and compare the read-only attribute | 0.25d |
| 2, 8, 9. Module split, Windows executor and recovery, four suites | Not prototyped. The executor must adopt the open-then-rename semantics, which mostly simplify it because verification happens on the handle | 2.5-3d |

**Risks:**

- **Sharing violations.** Indexers and antivirus hold files without `FILE_SHARE_DELETE`, so the
  source open fails until they let go.
- **Locked ancestors.** Users cannot rename the folders above an open project. This needs a
  clear UI message, not a code change.

macOS (M3 phase 2, 3-4 days) is also de-risked: `src/macos.rs` is the Linux module with
`renameatx_np` and a move-back for the checked rename, and it passed CI on its first run.
