//! Writes `htmlapp.d.ts` to stdout. `htmlapp types` uses the same emitter.
fn main() {
    print!("{}", htmlapp_bridge::emit_typescript());
}
