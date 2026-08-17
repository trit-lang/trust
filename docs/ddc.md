# Diverse Double-Compiling

What `bootstrap/` is *for*, stated once so that the work it needs is work
someone can plan.

Diverse Double-Compiling is David A. Wheeler's technique (2005, completed in
his 2009 dissertation), and it is the only practical answer anyone has to the
attack Ken Thompson described in *Reflections on Trusting Trust*: a compiler
binary that inserts a backdoor into what it compiles, and inserts the
inserter into any compiler it compiles, so that the source of both the
program and the compiler is clean and the backdoor survives anyway. Reading
the source cannot find it. Rebuilding the compiler from that source with
*itself* cannot find it, because that is exactly the step the attack rides.

---

## 1. What the technique is

Write `cA` for the compiler binary under suspicion, `sA` for the source it
claims to be, and `cP` for some **other** compiler for the same language —
the *parent*, which is trusted for this argument and need not be trusted for
anything else.

```
stage1 = cP(sA)          # sA compiled by the parent
stage2 = stage1(sA)      # sA compiled by that
```

`stage2` is a compiler built entirely from `sA` by something that is not
`cA`. If `stage2` and `cA` are **bit-for-bit identical**, then whatever `cA`
does, `sA` says it. A backdoor in `cA` that is not in `sA` would have to
survive a path it never touched, which it cannot.

The result is *corroboration*, not proof, and it is relative to `cP`: if the
parent is compromised in exactly the same way, the comparison passes and says
nothing. That is why the P stands for a compiler chosen to be **diverse** —
different implementation, different author, ideally a different lineage.

---

## 2. Why this repository is unusually well placed

The thing DDC needs and almost nobody has is a *second implementation of the
same language that agrees about everything*. This repository is building one
for a different reason — the differential invariant, which has found every
bug in `docs/spec-gaps.md` from G9.43 onward — and the two are the same
artifact:

| DDC wants | this repository has |
|---|---|
| `sA`, the compiler's source in its own language | `bootstrap/`, growing toward it |
| `cP`, a diverse compiler for that language | `trustc`, written in Rust |
| the two to agree about the language | `scripts/bootstrap.sh`, question by question |
| output that is a function of input | `scripts/reproducible.sh` |

The differential invariant is **not** DDC and does not replace it. It asks
whether two *sources* agree; DDC asks whether a *binary* is what its source
says. They are complementary, and this repository is one of the few places
where the second is reachable at all, because the first was built first.

---

## 3. What must be true before it can be run

These are requirements on the compiler, not on the ceremony, and each is a
piece of work.

### 3.1 The compiler is a function of its input — **holds**

Two runs must give the same bytes. `scripts/reproducible.sh` checks 32
compilations, each twice, in **separate processes** — which is the point,
because Rust seeds every `HashMap` differently per process, so iteration
order reaching the output shows up there as a difference. Nothing does
today.

What could break it later, and so is worth naming: a timestamp or a path in
the output, a name derived from a memory address, a parallel pass that emits
in completion order, and any iteration over a hash whose result is written
out rather than looked up.

### 3.2 `bootstrap/` compiles Trust — **in progress**

It reads the language (lexer, parser, Ch. 6's three passes), and it answers
Ch. 2 and the beginnings of Ch. 4. It does not yet lower to TIR, so there is
no `stage1` to build. The order of the remaining work is the order of the
chapters: finish Ch. 4's checking, then lowering, then the backend TIR
already has.

### 3.3 The comparison has something to compare — **not started**

`stage2` and `cA` must be *the same kind of thing*. The natural artifact here
is the **TIR module**, which has a canonical textual form (`trustc fmt`) and
is the compiler's real output; the assembly and the image are downstream of
it and are compared by the pipeline tests already. So the DDC comparison is:

```
stage1.tir = trustc build bootstrap/main.tr        # cP(sA)
stage2.tir = stage1 build bootstrap/main.tr        # stage1(sA)
```

and `stage2.tir` must equal `stage1.tir` byte for byte. That equality is the
**fixpoint**: a self-hosting compiler compiled by itself reproduces itself.

### 3.4 The parent must be diverse — **partly, and honestly not**

`trustc` is Rust and `bootstrap/` is Trust: different languages, different
authors of the *runtime* beneath them. But both were written here, by the
same hands, from the same specification. A backdoor placed in both by
whoever wrote them would survive DDC exactly as Thompson's survives a
rebuild.

What this repository can honestly claim once §3.3 holds is the weaker and
still worthwhile statement: **the Trust compiler's binary is what its Trust
source says, given `trustc`**. Making the claim stronger needs a `cP` this
project did not write — a third implementation, by someone else, from
`spec/`. That the specification is the authority, and is complete enough to
implement from, is what would make such a thing possible; it is one of the
reasons `spec/` is written the way it is.

### 3.5 The environment must be pinned

`cP` must be built from a known compiler, on a known libc, with known flags.
Today `trustc` needs a Rust toolchain and nothing else — zero external
crates, which is a decision made for other reasons and pays here too. The
remaining variable is `rustc` itself, and the honest statement is that the
root of trust is `rustc`'s, not ours. DDC pushes the question up the chain;
it does not end it.

---

## 4. The plan

Each step is checkable on its own, and none of them is only for DDC.

1. **Keep §3.1 true.** `scripts/reproducible.sh` runs with the rest.
   *Done, and now guarded.*
2. **Finish the checker** (Ch. 4 in `bootstrap/`): the types of expressions,
   then whether they agree — the first place the Trust side must say *no*.
   *Begun: `trust types` and `trust agree` are both compared, the second by
   rule rather than by wording or position, since `bootstrap/`'s tree
   carries no spans.*
3. **Lower to TIR** in Trust, compared against `trustc build` the way every
   other pass is compared: same input, same module, character for character.
   This is the step that makes `stage1` exist.

   What that comparison is, exactly, now that the rest is in place:

   ```
   $ trustc build tiny.tr
   tir 0.1 target "tritium"

   fn @add(%a: t27, %b: t27) -> t27 {
   ^entry:
       %a.slot.1 = slot tryte[3]
       %b.slot.2 = slot tryte[3]
       store t27 %a, %a.slot.1
       store t27 %b, %b.slot.2
       %v.3 = load t27 %a.slot.1
       %v.4 = load t27 %b.slot.2
       %a.5 = add.trap t27 %v.3, %v.4
       ret %a.5
   }
   ```

   The second implementation has to reproduce **the names too** — `%v.3`,
   `%a.5` — because equality is on the text. That is a stricter demand than
   it looks and it is the right one: the counter is per function and
   deterministic, so two implementations that agree about the *order* of
   what they emit agree about the names for free, and two that do not are
   two that emit different code. A comparison that normalized the names away
   would be a comparison that could not see the difference.

   The order to build it in is the order this file's corpus grew: scalars
   and arithmetic first, then calls, then blocks and `br3`, then aggregates,
   then the drops Ch. 3 §1.4 puts at the end of a scope.

   *Begun.* `bootstrap/lower.tr` emits the first slice — parameters, a `let`
   of a scalar, Ch. 1's arithmetic, a call, a `return`, a tail — and
   `scripts/bootstrap.sh` compares it against `trustc build` character for
   character, names included. A function it does not lower yet is left out
   rather than lowered wrongly, which is what keeps the comparison honest
   while the slice is small.
4. **Run the double compile.** `scripts/ddc.sh`: build `stage1` with
   `trustc`, build `stage2` with `stage1`, demand `stage2 == stage1`. Report
   the two hashes whether or not they match, because a number that is only
   printed when it is right is a number nobody checks.
5. **Write down what it proves.** A `DDC` section in `README.md` stating the
   claim, its parent, and its limits — including §3.4. A corroboration
   presented as a proof is worse than none.

---

## 5. What this changes about the work already planned

Nothing is dropped, but three things acquire a second reason:

- **`bootstrap/` must compile, not only read.** Reading Trust makes it a
  second opinion; compiling Trust makes it `sA`. The differential invariant
  is satisfied by the first, and DDC needs the second.
- **Canonical output matters more than it did.** `trustc fmt`'s canonical
  form was for reading diffs; it becomes the thing equality is *defined* on.
  Anything that makes two equivalent modules print differently — a name from
  a counter that depends on visit order, a set printed unsorted — is now a
  correctness bug rather than an untidiness.
- **The specification is the artifact that lets someone else write `cP`.**
  Every gap recorded in `docs/spec-gaps.md` is a place where a third
  implementation would have to guess, and a guess is a divergence that would
  read as a failed DDC. The gap file was for honesty; it is now also the
  work list for making the language implementable by a stranger.
