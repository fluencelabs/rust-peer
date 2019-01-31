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

import java.nio.file.Files

import cats.effect._
import com.softwaremill.sttp.asynchttpclient.cats.AsyncHttpClientCatsBackend
import com.softwaremill.sttp.circe.asJson
import com.softwaremill.sttp.{SttpBackend, _}
import fluence.node.config.{Configuration, MasterConfig}
import fluence.node.status.{MasterStatus, StatusAggregator}
import org.scalatest.{Timer ⇒ _, _}
import slogging.MessageFormatter.DefaultPrefixFormatter
import slogging.{LazyLogging, LogLevel, LoggerConfig, PrintLoggerFactory}

import scala.concurrent.ExecutionContext.Implicits.global
import scala.concurrent.duration._
import scala.language.higherKinds

/**
 * This test contains a single test method that checks:
 * - MasterNode connectivity with ganache-hosted Fluence smart contract
 * - MasterNode ability to load previous node clusters and subscribe to new clusters
 * - Successful cluster formation and starting blocks creation
 */
class MasterNodeSpec
    extends WordSpec with LazyLogging with Matchers with BeforeAndAfterAll with OptionValues with Integration
    with GanacheSetup {

  implicit private val ioTimer: Timer[IO] = IO.timer(global)
  implicit private val ioShift: ContextShift[IO] = IO.contextShift(global)

  private val sttpResource: Resource[IO, SttpBackend[IO, Nothing]] = Resource
    .make(IO(AsyncHttpClientCatsBackend[IO]()))(sttpBackend ⇒ IO(sttpBackend.close()))

  override protected def beforeAll(): Unit = {
    wireupContract()
  }

  override protected def afterAll(): Unit = {
    killGanache()
  }

  def getStatus(statusPort: Short)(implicit sttpBackend: SttpBackend[IO, Nothing]): IO[MasterStatus] = {
    import MasterStatus._
    for {
      resp <- sttp.response(asJson[MasterStatus]).get(uri"http://127.0.0.1:$statusPort/status").send()
    } yield {
      resp.unsafeBody.right.get
    }
  }

  "MasterNode" should {
    PrintLoggerFactory.formatter = new DefaultPrefixFormatter(false, false, false)
    LoggerConfig.factory = PrintLoggerFactory()
    LoggerConfig.level = LogLevel.DEBUG

    "provide status" in {
      val masterConf =
        MasterConfig.load().unsafeRunSync().copy(tendermintPath = Files.createTempDirectory("masternodespec").toString)
      val Configuration(rootPath, nodeConf) = Configuration.init(masterConf).unsafeRunSync()

      val resource = for {
        sttpB ← sttpResource
        node ← {
          implicit val s = sttpB
          MasterNode
            .resource[IO, IO.Par](masterConf, nodeConf, rootPath)
        }
        _ ← StatusAggregator.makeHttpResource(masterConf, node)
      } yield (sttpB, node)

      resource.use {
        case (sttpB, node) ⇒
          implicit val s = sttpB
          for {
            fiber <- Concurrent[IO].start(node.run)
            _ = println("Node Run")
            _ ← eventually[IO](getStatus(5678).map(println), 1.second, 15.seconds)
            _ ← fiber.join
          } yield ()

      }.unsafeRunSync()

    }
  }
}
