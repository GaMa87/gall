fn calc() -> u32 {
    let a = { let x = 1; x + 1 };
    let b = { let x = 2; x * 3 };
    a + b
}
