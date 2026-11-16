package dev.zerodroid.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.zerodroid.core.nativebridge.NativeRuntime

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background
                ) {
                    ZeroDroidHome()
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ZeroDroidHome() {
    var runtimeReport by remember {
        mutableStateOf(
            runCatching { "Native ABI ${NativeRuntime.abiVersion()}" }
                .getOrElse { "Native runtime unavailable: ${it.javaClass.simpleName}" }
        )
    }

    Scaffold(
        topBar = { TopAppBar(title = { Text("ZeroDroid") }) }
    ) { contentPadding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(
                start = 20.dp,
                top = contentPadding.calculateTopPadding() + 20.dp,
                end = 20.dp,
                bottom = contentPadding.calculateBottomPadding() + 24.dp
            ),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            item {
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(
                        modifier = Modifier.padding(20.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp)
                    ) {
                        Text("Android 虚拟机", style = MaterialTheme.typography.headlineSmall)
                        Text(
                            "独立 Guest Kernel、Android Framework 与虚拟硬件运行环境。",
                            style = MaterialTheme.typography.bodyMedium
                        )
                        Text(
                            "应用层采用 Kotlin、Jetpack Compose 与原生 Material 3。",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                    }
                }
            }
            item {
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(
                        modifier = Modifier.padding(20.dp),
                        verticalArrangement = Arrangement.spacedBy(12.dp)
                    ) {
                        Text("Native Runtime", style = MaterialTheme.typography.titleLarge)
                        Text(runtimeReport, style = MaterialTheme.typography.bodyMedium)
                        Button(
                            onClick = {
                                runtimeReport = runCatching {
                                    NativeRuntime.probeHostCapabilities().toString()
                                }.getOrElse {
                                    "Capability probe unavailable: ${it.javaClass.simpleName}: ${it.message.orEmpty()}"
                                }
                            }
                        ) {
                            Text("检测宿主虚拟化能力")
                        }
                    }
                }
            }
        }
    }
}
