fn shadow(x: u32) -> u32 {
    let y = {
        let x = x + 1;
        x * 2
    };
    x + y
}
