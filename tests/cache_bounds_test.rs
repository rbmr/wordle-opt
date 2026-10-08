use wordle_opt::cache::GlobalCache;

#[test]
fn test_global_cache_extreme_values() {
    let cache = GlobalCache::new(1024);

    // Test that the maximum possible value 262143 (which is (1<<18) - 1) doesn't overflow.
    let max_val = (1 << 18) - 1;
    cache.insert(12345, max_val, true);

    let res = cache.get(12345);
    assert!(res.is_some());
    let (val, exact) = res.unwrap();
    assert_eq!(val, max_val);
    assert_eq!(exact, true);
}
