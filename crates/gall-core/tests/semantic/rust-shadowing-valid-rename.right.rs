fn shadow(input: u32) -> u32 {
    let doubled = {
        let next = input + 1;
        next * 2
    };
    input + doubled
}
