fn main() {
    #[cfg(feature = "redis-store")]
    {
        #[cfg(not(any(feature = "bincode", feature = "messagepack")))]
        compile_error!(
            "redis-store feature requires either 'bincode' or 'messagepack' feature to be enabled"
        );

        #[cfg(all(feature = "bincode", feature = "messagepack"))]
        compile_error!("Cannot enable both 'bincode' and 'messagepack' features simultaneously");
    }
}
