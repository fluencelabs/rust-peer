import SbtCommons._
import sbt.Keys._
import sbt._

import scala.sys.process._

name := "fluence"

commons

initialize := {
  val _ = initialize.value // run the previous initialization
  val required = "1.8" // counter.wast cannot be run under Java 9. Remove this check after fixes.
  val current = sys.props("java.specification.version")
  assert(current == required, s"Unsupported JDK: java.specification.version $current != $required")
}

/* Projects */

lazy val vm = (project in file("vm"))
  .settings(
    commons,
    libraryDependencies ++= Seq(
      "com.github.cretz.asmble" % "asmble-compiler" % "0.4.0-fl",
      cats,
      catsEffect,
      pureConfig,
      cryptoHashing,
      scalaTest,
      mockito
    )
  )
  .enablePlugins(AutomateHeaderPlugin)

lazy val `vm-counter` = (project in file("vm/examples/counter"))
  .settings(
    commons,
    // we have to build fat jar because is not possible to simply run [[CounterRunner]]
    // with sbt (like sbt vm-counter/run) because sbt uses custom ClassLoader.
    // 'Asmble' code required for loading some classes (like RuntimeHelpers)
    // only with system ClassLoader.
    assemblyJarName in assembly := "counter.jar",
    // override `run` task
    run := {
      val log = streams.value.log
      log.info("Compiling counter.rs to counter.wasm and running with Fluence.")

      val scalaVer = scalaVersion.value.slice(0, scalaVersion.value.lastIndexOf("."))
      val projectRoot = file("").getAbsolutePath
      val cmd = s"sh vm/examples/run_example.sh counter $projectRoot $scalaVer"

      log.info(s"Running $cmd")

      assert(cmd ! log == 0, "Compile Rust to Wasm failed.")
    }
  )
  .dependsOn(vm)
  .enablePlugins(AutomateHeaderPlugin)

lazy val `vm-sqldb` = (project in file("vm/examples/sqldb"))
  .settings(
    commons,
    assemblyJarName in assembly := "sqldb.jar",
    // override `run` task
    run := {
      val log = streams.value.log
      log.info("Compiling sqldb.rs to sqldb.wasm and running with Fluence.")

      val scalaVer = scalaVersion.value.slice(0, scalaVersion.value.lastIndexOf("."))
      val projectRoot = file("").getAbsolutePath
      val cmd = s"sh vm/examples/run_example.sh sqldb $projectRoot $scalaVer"

      log.info(s"Running $cmd")

      assert(cmd ! log == 0, "Compile Rust to Wasm failed.")
    }
  )
  .dependsOn(vm)
  .enablePlugins(AutomateHeaderPlugin)

lazy val statemachine = (project in file("statemachine"))
  .settings(
    commons,
    libraryDependencies ++= Seq(
      cats,
      catsEffect,
      circeGeneric,
      circeParser,
      pureConfig,
      slogging,
      scodecBits,
      "com.github.jtendermint" % "jabci"          % "0.17.1",
      "org.bouncycastle"       % "bcpkix-jdk15on" % "1.56",
      "net.i2p.crypto"         % "eddsa"          % "0.3.0",
      scalaTest
    ),
    test in assembly := {},
    dockerfile in docker := {
      // The assembly task generates a fat JAR file
      val artifact: File = assembly.value
      val artifactTargetPath = s"/app/${artifact.name}"
      val tmVersion = "0.23.0"
      val tmDataRoot = "/tendermint"

      new Dockerfile {
        from("xqdocker/ubuntu-openjdk:jre-8")
        run("apt", "-yqq", "update")
        run("apt", "-yqq", "install", "wget", "curl", "jq", "unzip", "screen")
        run("wget", s"https://github.com/tendermint/tendermint/releases/download/v${tmVersion}/tendermint_${tmVersion}_linux_amd64.zip")
        run("unzip", "-d", "/bin", s"tendermint_${tmVersion}_linux_amd64.zip")

        run("mkdir", tmDataRoot)
        expose(26656, 26657)
        //volume(tmDataRoot)

        run("tendermint", "init", s"--home=$tmDataRoot")

        add(artifact, artifactTargetPath)

        entryPoint("bash", "/container_data/run-node.sh", tmDataRoot, artifactTargetPath)
      }
    }
  )
  .enablePlugins(AutomateHeaderPlugin)
  .enablePlugins(DockerPlugin)
  .dependsOn(vm)

lazy val externalstorage = (project in file("externalstorage"))
  .settings(
    commons,
    libraryDependencies ++= Seq(
      cats,
      catsEffect,
      sttp,
      sttpCirce,
      sttpCatsBackend,
      slogging,
      circeCore,
      circeGeneric,
      circeGenericExtras,
      pureConfig,
      scodecBits,
      scodecCore,
      web3jCrypto,
      cryptoHashing,
      scalaTest
    )
  )
  .enablePlugins(AutomateHeaderPlugin)

lazy val ethclient = (project in file("ethclient"))
  .settings(
    commons,
    libraryDependencies ++= Seq(
      "org.web3j" % "core" % "3.5.0",
      slogging,
      scodecBits,
      cats,
      catsEffect,
      utest
    ),
    setUTestFramework
  )
  .enablePlugins(AutomateHeaderPlugin)
