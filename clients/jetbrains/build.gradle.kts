plugins {
    java
    id("org.jetbrains.intellij.platform")
}

group = "com.opencoding"
version = "0.1.0"

java {
    toolchain.languageVersion.set(JavaLanguageVersion.of(17))
}

dependencies {
    implementation("com.google.code.gson:gson:2.13.2")
    testImplementation(platform("org.junit:junit-bom:5.13.4"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("junit:junit:4.13.2")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")

    intellijPlatform {
        intellijIdea("2025.1.7")
    }
}

intellijPlatform {
    buildSearchableOptions = false
    pluginConfiguration {
        ideaVersion {
            sinceBuild = "251"
            untilBuild = "263.*"
        }
    }
    pluginVerification {
        ides {
            current()
        }
    }
}

tasks.test {
    useJUnitPlatform()
}
