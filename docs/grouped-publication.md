# Bounded group publication

Directory and URL-list workers prepare exact final Markdown bytes while retaining
the native output claim. Preparation is provisional: it does not emit success,
complete recovery state, or write the image metadata index. Assets and screenshots
can already exist, as in the immediate conversion path. The core's owned prepared
result retains paid usage even when enhancement or publication fails. Single-file
conversion and language bindings retain their immediate publication path.

The coordinator groups at most 16 documents and 64 MiB of rendered Markdown.
Each document has one combined receipt for its base and optional enhanced member.
Empty and oversized plans publish through the immediate path without converting
or requesting model work again. A full group, end of work, interruption, fatal
coordinator error or a 100 ms age observed by the coordinator triggers draining.
That age is a scheduling target, not a wall-clock completion guarantee.

Three ordered durability phases precede acknowledgement:

1. Sync every staged document and receipt, including their temporary directory
   entries, then complete the media fence.
2. Install all receipts and sync their directories, then complete the media fence.
3. Install all document members and sync their directories, then complete the
   media fence. Only now finalize image indexes and terminal results.

Every phase retains the same claims. The scheduler independently owns each claim
through terminal recording, including when a group operation fails. Failures may
leave some installed receipts or document members; they do not roll back the
whole group or report provisional successes. Durable receipts authorize only
exact prior/prepared proofs during a later retry. Modified foreign files and
substituted temporary names remain protected by the existing ownership checks.

On a verified local macOS APFS/HFS volume, each object receives ordinary `fsync`,
followed by `F_FULLFSYNC` through a retained descriptor on that volume for each
phase. The inference is that the full device cache flush covers the preceding
completed object flushes. Apple's [storage session](https://developer.apple.com/videos/play/wwdc2019/419/)
describes the distinction between OS-cache synchronization and flushing a drive's
cache; its [fcntl manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html)
describes `F_FULLFSYNC`. This uses a full flush, not `F_BARRIERFSYNC` ordering.
Other filesystems/platforms retain per-object `File::sync_all`. Each phase holds
at most 32 volume descriptors; additional volumes use the per-object path.
An error fails the group, without a weaker success fallback.

The guarantee remains conditional on filesystem and hardware flush behavior.
Injected fence failures and real process-kill/reopen tests exercise ordering and
ownership; they cannot simulate physical loss of drive power. Existing directory
and claim initialization fences are retained. The 64 MiB limit bounds staged
Markdown, not total process memory or active conversion memory.

Admission persistence is separately batched before dispatch; see
[recovery state](state-storage.md). Worker concurrency remains configurable,
including values above 16. Publication groups remain bounded independently.
Performance gains require fresh equivalent-output measurements; the old directory
regression is not closed by source-level syscall accounting.
