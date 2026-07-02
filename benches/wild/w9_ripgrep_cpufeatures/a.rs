fn runtime_cpu_features() -> Vec<String> {
    #[cfg(target_arch = "x86_64")]
    {
        let mut features = vec![];

        let sse2 = is_x86_feature_detected!("sse2");
        features.push(format!("{sign}SSE2", sign = sign(sse2)));

        let ssse3 = is_x86_feature_detected!("ssse3");
        features.push(format!("{sign}SSSE3", sign = sign(ssse3)));

        let avx2 = is_x86_feature_detected!("avx2");
        features.push(format!("{sign}AVX2", sign = sign(avx2)));

        features
    }
    #[cfg(target_arch = "aarch64")]
    {
        let mut features = vec![];

        // memchr and aho-corasick only use NEON when it is available at
        // compile time. This isn't strictly necessary, but NEON is supposed
        // to be available for all aarch64 targets. If this isn't true, please
        // file an issue at https://github.com/BurntSushi/memchr.
        let neon = cfg!(target_feature = "neon");
        features.push(format!("{sign}NEON", sign = sign(neon)));

        features
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        vec![]
    }
}
