import Lean
open Lean

/-! Compatibility shim for Lean 4.16.x–4.20.x (tested: 4.16.0).

`importModules` in these versions always runs the builtin environment
extensions' import hooks (there is no `loadExts` flag). Those hooks can
evaluate constants of the imported modules with the interpreter (for example
registered delaborators and macros), so on these versions the export must
run inside the sandbox. -/

namespace SlimExport.Compat

def variant : String := "legacy (Lean 4.16–4.20): importModules finalizes builtin extensions"
def rawModeSupported : Bool := false
def oleanLevel : String := "n/a"

/-- `loadExts` is ignored: these versions always load extensions. -/
def importEnv (imports : Array Import) (_loadExts : Bool) : IO Environment :=
  importModules imports {} 0

def importFlags (i : Import) : Array String :=
  if i.runtimeOnly then #["runtime"] else #[]

end SlimExport.Compat
