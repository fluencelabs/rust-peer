package fluence.node

import java.io.File

import cats.Traverse
import cats.effect.IO
import cats.syntax.show._
import com.typesafe.config.{ Config, ConfigFactory }
import fluence.crypto.{ FileKeyStorage, SignAlgo }
import fluence.crypto.algorithm.Ecdsa
import fluence.crypto.hash.{ CryptoHasher, JdkCryptoHasher }
import fluence.crypto.keypair.KeyPair
import fluence.kad.protocol.{ Contact, KademliaRpc, Key, Node }
import fluence.storage.rocksdb.RocksDbStore
import monix.eval.Task
import cats.instances.list._
import fluence.crypto.signature.SignatureChecker
import fluence.kad.Kademlia
import monix.execution.Scheduler

import scala.concurrent.duration._

trait FluenceNode {
  def config: Config

  def node: Task[Node[Contact]] = kademlia.ownContact

  def contact: Task[Contact] = node.map(_.contact)

  def kademlia: Kademlia[Task, Contact]

  def stop: IO[Unit]

  def restart: IO[FluenceNode]
}

object FluenceNode extends slogging.LazyLogging {

  /**
   * Launches a node with all available and enabled network interfaces.
   *
   * @param algo Algorithm to use for signatures
   * @param hasher Hasher, used in b-tree
   * @param config Configuration to read from
   * @return An IO that can be used to shut down the node
   */
  def startNode(
    algo: SignAlgo = Ecdsa.signAlgo,
    hasher: CryptoHasher[Array[Byte], Array[Byte]] = JdkCryptoHasher.Sha256,
    config: Config = ConfigFactory.load()): IO[FluenceNode] =
    launchGrpc(algo, hasher, config)

  /**
   * Initiates a directory with all its parents
   *
   * @param path Directory path to create
   * @return Existing directory
   */
  private def initDirectory(path: String): IO[File] =
    IO {
      val appDir = new File(path)
      if (!appDir.exists()) {
        appDir.getParentFile.mkdirs()
        appDir.mkdir()
      }
      appDir
    }

  /**
   * Generates or loads keypair
   *
   * @param keyPath Path to store keys in
   * @param algo Sign algo
   * @return Keypair, either loaded or freshly generated
   */
  private def getKeyPair(keyPath: String, algo: SignAlgo): IO[KeyPair] = {
    val keyFile = new File(keyPath)
    val keyStorage = new FileKeyStorage[IO](keyFile)
    keyStorage.getOrCreateKeyPair(algo.generateKeyPair[IO]().value.flatMap(IO.fromEither))
  }

  // TODO: move config reading stuff somewhere
  case class SeedsConfig(
      seeds: List[String]
  ) {
    def contacts(implicit checker: SignatureChecker): IO[List[Contact]] =
      Traverse[List].traverse(seeds)(s ⇒
        Contact.readB64seed[IO](s).value.flatMap(IO.fromEither)
      )
  }

  /**
   * Reads seed nodes contacts from config
   */
  def readSeedsConfig(conf: Config): IO[SeedsConfig] =
    IO {
      import net.ceedubs.ficus.Ficus._
      import net.ceedubs.ficus.readers.ArbitraryTypeReader._
      conf.as[SeedsConfig]("fluence.node.join")
    }

  /**
   * Launches GRPC node, using all the provided configs.
   * @return IO that will shutdown the node once ran
   */
  private def launchGrpc(algo: SignAlgo, hasher: CryptoHasher[Array[Byte], Array[Byte]], config: Config): IO[FluenceNode] = {
    import algo.checker
    for {
      _ ← initDirectory(config.getString("fluence.directory"))
      kp ← getKeyPair(config.getString("fluence.keyPath"), algo)
      key ← Key.fromKeyPair[IO](kp)

      builder ← NodeGrpc.grpcServerBuilder(config)
      contact ← NodeGrpc.grpcContact(algo.signer(kp), builder)

      client ← NodeGrpc.grpcClient(key, contact, config)
      kadClient = client.service[KademliaRpc[Task, Contact]] _

      cacheStore ← RocksDbStore[IO](config.getString("fluence.contract.cacheDirName"), config)
      services ← NodeComposer.services(kp, contact, algo, hasher, cacheStore, kadClient, config, acceptLocal = true)

      server ← NodeGrpc.grpcServer(services, builder, config)

      _ ← server.start

      seedConfig ← readSeedsConfig(config)
      seedContacts ← seedConfig.contacts

      _ ← if (seedContacts.nonEmpty) services.kademlia.join(seedContacts, 10).toIO(Scheduler.global) else IO{
        logger.info("You should add some seed node contacts to join. Take a look on reference.conf")
      }
    } yield {
      sys.addShutdownHook {
        logger.warn("*** shutting down gRPC server since JVM is shutting down")
        server.shutdown.unsafeRunTimed(5.seconds)
        logger.warn("*** server shut down")
      }

      logger.info("Server launched")
      logger.info("Your contact is: " + contact.show)

      logger.info("You may share this seed for others to join you: " + Console.MAGENTA + contact.b64seed + Console.RESET)

      val _conf = config

      new FluenceNode {
        override def config: Config = _conf

        override def kademlia: Kademlia[Task, Contact] = services.kademlia

        override def stop: IO[Unit] = server.shutdown

        override def restart: IO[FluenceNode] =
          stop.flatMap(_ ⇒ launchGrpc(algo, hasher, _conf))
      }
    }
  }

}
