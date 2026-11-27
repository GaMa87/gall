fn shadow_bug() -> u32 {
    let x = 1;
    let r = { let y = 2; x };
    r
}
