plugins { alias(libs.plugins.android.library) }
android { namespace = "dev.zerodroid.core.runtime"; compileSdk = 37; defaultConfig { minSdk = 28 } }
dependencies { api(project(":core:common")); api(project(":core:model")); api(project(":core:native-bridge")); api(libs.kotlinx.coroutines.core); testImplementation(libs.junit) }
