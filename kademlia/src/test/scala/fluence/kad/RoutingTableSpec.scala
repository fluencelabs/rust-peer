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

package fluence.kad

import cats.data.StateT
import cats.effect.{ContextShift, IO, Timer}
import fluence.kad.mvar.ReadableMVar
import fluence.kad.protocol.{KademliaRpc, Key, Node}
import org.scalatest.{Matchers, WordSpec}
import scodec.bits.ByteVector

import scala.concurrent.duration._
import scala.concurrent.ExecutionContext.global
import scala.language.implicitConversions

class RoutingTableSpec extends WordSpec with Matchers {
  implicit def key(i: Long): Key =
    Key.fromBytes.unsafe(Array.concat(Array.ofDim[Byte](Key.Length - java.lang.Long.BYTES), {
      ByteVector.fromLong(i).toArray
    }))

  implicit def toLong(k: Key): Long = {
    k.value.toLong()
  }

  private val pingDuration = Duration.Undefined

  implicit val shift: ContextShift[IO] = IO.contextShift(global)
  implicit val timer: Timer[IO] = IO.timer(global)

  "kademlia routing table (non-iterative)" should {

    val failLocalRPC = (_: Long) ⇒
      new KademliaRpc[Long] {
        override def ping() = IO.raiseError(new NoSuchElementException)

        override def lookup(key: Key, numberOfNodes: Int) = ???
        override def lookupAway(key: Key, moveAwayFrom: Key, numberOfNodes: Int) = ???
      }

    val successLocalRPC = (c: Long) ⇒
      new KademliaRpc[Long] {
        override def ping() = IO(Node(c, c))

        override def lookup(key: Key, numberOfNodes: Int) = ???
        override def lookupAway(key: Key, moveAwayFrom: Key, numberOfNodes: Int) = ???
      }

    val checkNode: Node[Long] ⇒ IO[Boolean] = _ ⇒ IO(true)

    def bucketOps(maxBucketSize: Int): BucketsState[IO, Long] =
      new BucketsState[IO, Long] {
        private val buckets = collection.mutable.Map.empty[Int, Bucket[Long]]

        override protected def run[T](bucketId: Int, mod: StateT[IO, Bucket[Long], T]) =
          mod.run(buckets.getOrElseUpdate(bucketId, Bucket(maxBucketSize))).map {
            case (b, v) ⇒
              buckets(bucketId) = b
              v
          }

        override def read(bucketId: Int) =
          IO(buckets.getOrElseUpdate(bucketId, Bucket(maxBucketSize)))

        override def toString: String =
          buckets.toString()
      }

    def siblingsOps(nodeId: Key, maxSiblingsSize: Int): SiblingsState[IO, Long] =
      new SiblingsState[IO, Long] {
        private val state = ReadableMVar.of[IO, Siblings[Long]](Siblings[Long](nodeId, maxSiblingsSize)).unsafeRunSync()

        override protected def run[T](mod: StateT[IO, Siblings[Long], T]) =
          state.apply(mod)

        override def read =
          state.read

        override def toString: String =
          state.read.unsafeRunSync().toString
      }

    "not fail when requesting its own key" in {
      val nodeId: Key = 0L
      val bo = bucketOps(2)
      val so = siblingsOps(nodeId, 2)
      val rt = new RoutingTable[IO, IO.Par, Long](nodeId, so, bo)

      rt.find(0L).unsafeRunSync() shouldBe empty
      rt.lookup(0L, 1).unsafeRunSync() shouldBe empty
    }

    "find nodes correctly" in {

      val nodeId: Key = 0L
      val bo = bucketOps(2)
      val so = siblingsOps(nodeId, 2)
      val rt = new RoutingTable[IO, IO.Par, Long](nodeId, so, bo)

      (1L to 5L).foreach { i ⇒
        rt.update(Node(i, i), failLocalRPC, pingDuration, checkNode).unsafeRunSync()
        (1L to i).foreach { n ⇒
          rt.find(n).unsafeRunSync() shouldBe defined
        }
      }

      rt.find(4L).unsafeRunSync() shouldBe defined

      rt.update(Node(6L, 6L), failLocalRPC, pingDuration, checkNode).unsafeRunSync() shouldBe true

      rt.find(4L).unsafeRunSync() shouldBe empty
      rt.find(6L).unsafeRunSync() shouldBe defined

      rt.update(Node(4L, 4L), successLocalRPC, pingDuration, checkNode).unsafeRunSync() shouldBe false

      rt.find(4L).unsafeRunSync() shouldBe empty
      rt.find(6L).unsafeRunSync() shouldBe defined

      rt.update(Node(4L, 4L), failLocalRPC, pingDuration, checkNode).unsafeRunSync() shouldBe true

      rt.find(4L).unsafeRunSync() shouldBe defined
      rt.find(6L).unsafeRunSync() shouldBe empty

      so.read.unsafeRunSync().nodes.toList.map(_.contact) shouldBe List(1L, 2L)

    }

    "lookup nodes correctly" in {
      val nodeId: Key = 0L
      val bo = bucketOps(2)
      val so = siblingsOps(nodeId, 10)
      val rt = new RoutingTable[IO, IO.Par, Long](nodeId, so, bo)

      (1L to 10L).foreach { i ⇒
        rt.update(Node(i, i), successLocalRPC, pingDuration, checkNode).unsafeRunSync()
      }

      val nbs10 = rt.lookup(100L, 10).unsafeRunSync()
      nbs10.size should be >= 7

      (1L to 127L).foreach { i ⇒
        rt.update(Node(i, i), successLocalRPC, pingDuration, checkNode).unsafeRunSync()
      }

      (1L to 127L).foreach { i ⇒
        rt.lookup(i, 100).unsafeRunSync().size should be >= 10
      }

      so.read.unsafeRunSync().nodes.toList.map(_.contact) shouldBe (1L to 10L).toList
    }
  }
}
