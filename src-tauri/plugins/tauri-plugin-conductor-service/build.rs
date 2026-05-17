fn main() {
    tauri_plugin::Builder::new(&[
        "start_service",
        "stop_service",
        "request_battery_exemption",
        "is_battery_optimized",
        "is_shared_conductor_available",
    ])
    .android_path("android")
    .build();
}
