import Lean
open Lean

/-! Compatibility shim for Lean 4.21.x–4.27.x (tested: 4.21.0).

Uses `importModules (level := .private)` so that private data (theorem bodies
of module-system files) is loaded. With `--pp raw`, `loadExts := false`: the
environment-extension import hooks are not run and no code from the imported
modules is evaluated. By default (`--pp notation`) extensions are loaded after
`enableInitializersExecution`, which runs the imported modules' initializers
and attribute handlers (needed for notation such as `=` and `∃` when
pretty-printing); this is project code and must run in the sandbox. -/

namespace SlimExport.Compat

def variant : String := "mid (Lean 4.21–4.27): importModules level:=private"
def rawModeSupported : Bool := true
def oleanLevel : String := "private"

def importEnv (imports : Array Import) (loadExts : Bool) : IO Environment :=
  importModules imports {} (trustLevel := 0) (loadExts := loadExts) (level := OLeanLevel.private)

def importFlags (i : Import) : Array String := Id.run do
  let mut fs : Array String := #[]
  if i.isExported then fs := fs.push "public"
  if i.importAll then fs := fs.push "all"
  return fs

end SlimExport.Compat
