/*
 * Copyright 2018 Fluence Labs Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package fluence.node

import java.nio.ByteBuffer

import cats.effect._
import cats.syntax.apply._
import com.softwaremill.sttp.asynchttpclient.fs2.AsyncHttpClientFs2Backend
import com.softwaremill.sttp.circe.asJson
import com.softwaremill.sttp.{SttpBackend, _}
import fluence.effects.ethclient.EthClient
import fluence.log.{Log, LogFactory}
import fluence.node.config.FluenceContractConfig
import fluence.node.eth.FluenceContractTestOps._
import fluence.node.eth.{FluenceContract, NodeEthState}
import fluence.node.status.MasterStatus
import org.scalatest.{Timer => _, _}
import eth.FluenceContractTestOps._
import fluence.log.{Log, LogFactory}
import fluence.node.config.FluenceContractConfig
import fluence.effects.testkit.Timed
import fluence.worker.WorkerStatus

import scala.concurrent.ExecutionContext.Implicits.global
import scala.concurrent.duration._
import scala.language.higherKinds
import scala.sys.process._

/**
 * This test contains a single test method that checks:
 * - MasterNode connectivity with ganache-hosted Fluence smart contract
 * - MasterNode ability to load previous node clusters and subscribe to new clusters
 * - Successful cluster formation and starting blocks creation
 */
class MasterNodeIntegrationSpec
    extends WordSpec with Matchers with BeforeAndAfterAll with OptionValues with Timed with TendermintSetup
    with GanacheSetup with DockerSetup {

  type Sttp = SttpBackend[IO, fs2.Stream[IO, ByteBuffer]]

  implicit private val ioTimer: Timer[IO] = IO.timer(global)
  implicit private val ioShift: ContextShift[IO] = IO.contextShift(global)

  implicit private val log: Log[IO] =
    LogFactory.forPrintln[IO](Log.Error).init("MasterNodeIntegrationSpec").unsafeRunSync()

  private val sttpResource: Resource[IO, SttpBackend[IO, fs2.Stream[IO, ByteBuffer]]] =
    Resource.make(IO(AsyncHttpClientFs2Backend[IO]()))(sttpBackend ⇒ IO(sttpBackend.close()))

  // TODO integrate with CLI, get app id from tx
  @volatile private var lastAppId = 1L

  override protected def beforeAll(): Unit = {
    wireupContract()
  }

  override protected def afterAll(): Unit = {
    killGanache()
  }

  def getStatus(statusPort: Short)(implicit sttpBackend: Sttp): IO[Either[RuntimeException, MasterStatus]] = {
    import MasterStatus._
    for {
      resp <- sttp.response(asJson[MasterStatus]).get(uri"http://127.0.0.1:$statusPort/status").send()
    } yield {
      resp.body.flatMap(_.left.map(e => s"${e.message} ${e.original} ${e.error}")).left.map(new RuntimeException(_))
    }
  }

  def getEthState(statusPort: Short)(implicit sttpBackend: Sttp): IO[NodeEthState] = {
    import MasterStatus._
    for {
      resp <- sttp.response(asJson[NodeEthState]).get(uri"http://127.0.0.1:$statusPort/status/eth").send()
    } yield {
      resp.unsafeBody.right.get
    }
  }

  def checkMasterRunning(statusPort: Short)(implicit sttpBackend: Sttp): IO[Unit] =
    getEthState(statusPort).map(_.contractAppsLoaded shouldBe true)

  def runTwoMasters(basePort: Short)(implicit sttpBackend: Sttp): Resource[IO, Seq[String]] = {
    val master1Port: Short = basePort
    val master2Port: Short = (basePort + 1).toShort
    for {
      master1 <- runMaster(master1Port, "master1", n = 1)
      master2 <- runMaster(master2Port, "master2", n = 2)

      _ <- Resource liftF eventually[IO](
        checkMasterRunning(master1Port) *> checkMasterRunning(master2Port),
        maxWait = 45.seconds
      ) // TODO: 45 seconds is a bit too much for startup; investigate and reduce timeout

    } yield Seq(master1, master2)
  }

  def getRunningWorker(statusPort: Short)(implicit sttpBackend: Sttp): IO[Option[WorkerStatus]] =
    IO.suspend {
      getStatus(statusPort).map {
        case Right(st) =>
          st.workers.headOption.flatMap {
            case (_, w: WorkerStatus) if w.isOperating =>
              Some(w)
            case _ ⇒
              log.debug("Trying to get WorkerRunning, but it is not healthy in status: " + st).unsafeRunSync()
              None
          }
        case Left(e) =>
          log.error(s"Error on getting worker status at $statusPort: $e").unsafeRunSync()
          None
      }
    }

  def withEthSttpAndTwoMasters(basePort: Short): Resource[IO, (EthClient, Sttp)] =
    for {
      ethClient <- EthClient.make[IO]()
      implicit0(sttp: Sttp) <- sttpResource
      _ <- runTwoMasters(basePort)
    } yield (ethClient, sttp)

  "MasterNodes" should {
    val contractAddress = "0x9995882876ae612bfd829498ccd73dd962ec950a"
    val owner = "0x4180FC65D613bA7E1a385181a219F1DBfE7Bf11d"

    log.info(s"Docker host: '$dockerHost'").unsafeRunSync()

    val contractConfig = FluenceContractConfig(owner, contractAddress)

    def runTwoWorkers(basePort: Short)(implicit ethClient: EthClient, sttp: Sttp): IO[Unit] = {
      val contract = FluenceContract(ethClient, contractConfig)
      val master2Port = (basePort + 1).toShort
      for {
        status1 <- getStatus(basePort).map(_.toTry.get)
        status2 <- getStatus(master2Port).map(_.toTry.get)
        ethState1 <- getEthState(basePort)
        ethState2 <- getEthState(master2Port)

        _ <- contract.addNode[IO](ethState1.validatorKey, status1.ip, basePort, 1).attempt
        _ <- contract.addNode[IO](ethState2.validatorKey, status2.ip, master2Port, 1).attempt
        blockNumber <- contract.addApp[IO]("llamadb", clusterSize = 2)

        _ ← log.info("Added App at block: " + blockNumber + ", now going to wait for two workers")

        _ <- eventually[IO](
          for {
            status0 <- heightFromTendermintStatus("localhost", basePort, lastAppId)
            _ ← log.info(s"c1s0 === " + status0.sync_info.latest_block_height)
            status1 <- heightFromTendermintStatus("localhost", master2Port, lastAppId)
            _ ← log.info(s"c1s1 === " + status1.sync_info.latest_block_height)
          } yield {
            status0.sync_info.latest_block_height should be >= 2L
            status1.sync_info.latest_block_height should be >= 2L
          },
          maxWait = 90.seconds
        )

        _ = lastAppId += 1

        _ ← log.info("Height equals two for workers, going to get WorkerRunning from Master status")

        _ <- eventually[IO](
          for {
            worker1 <- getRunningWorker(basePort)
            worker2 <- getRunningWorker((basePort + 1).toShort)
          } yield {
            worker1 shouldBe defined
            worker2 shouldBe defined
          },
          maxWait = 90.seconds
        )
      } yield ()
    }

    def deleteApp(basePort: Short): IO[Unit] =
      withEthSttpAndTwoMasters(basePort).use {
        case (ethClient, s) =>
          log.debug("Prepared two masters for Delete App test").unsafeRunSync()
          implicit val sttp = s
          val getStatus1 = getRunningWorker(basePort)
          val getStatus2 = getRunningWorker((basePort + 1).toShort)
          val contract = FluenceContract(ethClient, contractConfig)

          for {
            _ <- runTwoWorkers(basePort)(ethClient, s)

            _ ← log.debug("Two workers should be running")

            // Check that we have 2 running workers from runTwoWorkers
            status1 <- getStatus1
            _ = status1 shouldBe defined
            _ ← log.debug("Worker1 running: " + status1)

            status2 <- getStatus2
            _ = status2 shouldBe defined
            _ ← log.debug("Worker2 running: " + status2)

            appId = 1L
            _ <- contract.deleteApp[IO](appId)
            _ ← log.debug("App deleted from contract")

            _ <- eventually[IO](
              for {
                s1 <- getStatus1
                s2 <- getStatus2
              } yield {
                // Check that workers run no more
                s1 should not be defined
                s2 should not be defined
              },
              maxWait = 30.seconds
            )
          } yield ()
      }

    "sync their workers with contract clusters" in {
      val basePort: Short = 20000

      withEthSttpAndTwoMasters(basePort).use {
        case (e, s) =>
          runTwoWorkers(basePort)(e, s).flatMap(_ ⇒ IO("docker ps".!!))
      }.flatMap { psOutput ⇒
        psOutput should include("worker")
        // Check that once masters are stopped, workers do not exist
        eventually(IO("docker ps -a".!! should not include "worker"))

      }.unsafeRunSync()
    }

    "stop workers on AppDelete event" in {
      deleteApp(21000).unsafeRunSync()
    }
  }
}
