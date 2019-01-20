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

package fluence.statemachine

import com.github.jtendermint.jabci.api.CodeType
import com.github.jtendermint.jabci.types.{RequestCheckTx, RequestCommit, RequestDeliverTx, RequestQuery}
import com.google.protobuf.ByteString
import fluence.statemachine.config.StateMachineConfig
import fluence.statemachine.state.QueryCodeType
import fluence.statemachine.tree.MerkleTreeNode
import fluence.statemachine.tx.{Computed, Error}
import fluence.statemachine.util.{ClientInfoMessages, HexCodec}
import org.scalatest.{Matchers, OneInstancePerTest, WordSpec}

class IntegrationSpec extends WordSpec with Matchers with OneInstancePerTest {

  // sbt defaults user directory to submodule directory
  // while Idea defaults to project root
  private val moduleDirPrefix = if (System.getProperty("user.dir").endsWith("/statemachine")) "../" else "./"
  private val moduleFiles = List("mul.wast", "counter.wast").map(moduleDirPrefix + "vm/src/test/resources/wast/" + _)
  private val config = StateMachineConfig(8, moduleFiles, "OFF")

  val abciHandler: AbciHandler = ServerRunner
    .buildAbciHandler(config)
    .valueOr(e => throw new RuntimeException(e.message))
    .unsafeRunSync()

  def sendCheckTx(tx: String): (Int, String) = {
    val request = RequestCheckTx.newBuilder().setTx(ByteString.copyFromUtf8(tx)).build()
    val response = abciHandler.requestCheckTx(request)
    (response.getCode, response.getInfo)
  }

  def sendDeliverTx(tx: String): (Int, String) = {
    val request = RequestDeliverTx.newBuilder().setTx(ByteString.copyFromUtf8(tx)).build()
    val response = abciHandler.receivedDeliverTx(request)
    (response.getCode, response.getInfo)
  }

  def sendCommit(): Unit =
    abciHandler.requestCommit(RequestCommit.newBuilder().build())

  def sendQuery(query: String, height: Int = 0): Either[(Int, String), String] = {
    val builtQuery = RequestQuery.newBuilder().setHeight(height).setPath(query).setProve(true).build()
    val response = abciHandler.requestQuery(builtQuery)
    response.getCode match {
      case CodeType.OK => Right(response.getValue.toStringUtf8)
      case _ => Left((response.getCode, response.getInfo))
    }
  }

  def latestCommittedHeight: Long = abciHandler.committer.stateHolder.latestCommittedHeight.unsafeRunSync()

  def latestCommittedState: MerkleTreeNode = abciHandler.committer.stateHolder.mempoolState.unsafeRunSync()

  def latestAppHash: String = latestCommittedState.merkleHash.toHex

  def tx(client: SigningClient, session: String, order: Long, payload: String, signature: String): String = {
    val txHeaderJson = s"""{"client":"${client.id}","session":"$session","order":$order}"""
    val txJson = s"""{"header":$txHeaderJson,"payload":"$payload","timestamp":"0"}"""
    val signedTxJson = s"""{"tx":$txJson,"signature":"$signature"}"""
    HexCodec.stringToHex(signedTxJson).toUpperCase
  }

  def tx(client: SigningClient, session: String, order: Long, payload: String): String = {
    val txHeaderJson = s"""{"client":"${client.id}","session":"$session","order":$order}"""
    val txJson = s"""{"header":$txHeaderJson,"payload":"$payload"}"""
    val signingData = s"${client.id}-$session-$order-$payload"
    val tt = client.sign(signingData)
    val signedTxJson = s"""{"tx":$txJson,"signature":"${client.sign(signingData)}"}"""
    HexCodec.stringToHex(signedTxJson).toUpperCase
  }

  def littleEndian4ByteHex(number: Int): String =
    Integer.toString(number, 16).reverse.padTo(8, '0').grouped(2).map(_.reverse).mkString.toUpperCase

  "State machine" should {
    val client = SigningClient(
      "client001",
      "TVAD4tNeMH2yJfkDZBSjrMJRbavmdc3/fGU2N2VAnxQ"
    )
    val session = "157A0E"
    val tx0 = tx(
      client,
      session,
      0,
      "()",
      "L20to3vLwexFgUC1XgOaCKKNCxo433ScYc+EKBQdnMpIqlUOifG4Vn/9qL1OhLpBUGKOYRFBi3l517Uu37mOAQ=="
    )
    val tx1 = tx(
      client,
      session,
      1,
      "MulModule(0A0000000E000000)",
      "WYiFrfG2qOhLzrVYl2c6twsIXqr92wxggd8t3+xeJtbIwE4cldX9K070X8ztNT5cVVLZ+Qd/tYMhsMlv7yLzDQ=="
    )
    val tx2 = tx(client, session, 2, "()")
    val tx3 = tx(client, session, 3, "()")
    val tx0Failed = tx(client, session, 0, "WrongModuleName()")
    val tx0Result = s"@meta/${client.id}/$session/0/result"
    val tx1Result = s"@meta/${client.id}/$session/1/result"
    val tx2Result = s"@meta/${client.id}/$session/2/result"
    val tx3Result = s"@meta/${client.id}/$session/3/result"

    "process correct tx/query sequence" in {
      // TODO: rewrite tests. 2 kinds of tests required:
      // 1. Many VM-agnostic tests for State machine logic would not depend on currently used VM impl (with VM stubbed).
      // 2. Few VM-SM tests (similar to the current one) integration tests that fix actual app_hash'es for integration.
      //
      // Currently only this test looks like integrational, other tests should be more decoupled from VM logic.

      sendCommit()
      sendCommit()
      latestAppHash shouldBe "2CA475E20CD8DE7612D3F2EE205AB3D8B2C4E541A0638591CDE5574524D6BFC3"

      sendCheckTx(tx0)
      sendCheckTx(tx1)
      sendCheckTx(tx2)
      sendCheckTx(tx3)
      sendQuery(tx1Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))
      sendDeliverTx(tx0)
      sendCommit()
      latestAppHash shouldBe "6E0B919DB0E8B2A7A877AA0DDF4D238379BC05906B36181B902417A006D1ABEE"

      sendCheckTx(tx1)
      sendCheckTx(tx2)
      sendCheckTx(tx3)
      sendQuery(tx1Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))
      sendDeliverTx(tx1)
      sendDeliverTx(tx2)
      sendDeliverTx(tx3)
      sendCommit()
      latestAppHash shouldBe "358E3349C85783100B55DEFA5F3C2500A7D73180545A93A4768E66D339EB0B21"

      sendQuery(tx1Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))
      sendCommit()

      sendQuery(tx1Result) shouldBe Right(Computed(littleEndian4ByteHex(140)).toStoreValue)
      sendQuery(tx3Result) shouldBe Right(Computed(littleEndian4ByteHex(3)).toStoreValue)

      latestCommittedHeight shouldBe 5
      latestAppHash shouldBe "358E3349C85783100B55DEFA5F3C2500A7D73180545A93A4768E66D339EB0B21"
    }

    "incorrect hex string in Tx payload argument" in {
      val txIncorrentArgument = tx(client, session, 0, "(asdsad)")

      sendCommit()
      sendCommit()

      sendDeliverTx(txIncorrentArgument)
      sendDeliverTx(tx1) shouldBe (CodeType.BAD, ClientInfoMessages.SessionAlreadyClosed)

      sendCommit()
      sendCommit()

      sendQuery(s"@meta/${client.id}/$session/0/result") shouldBe
        Right(Error("WrongPayloadArgument", "Wrong payload argument=(asdsad)").toStoreValue)
    }

    "parentheses is absent" in {
      val txLeftBracketAbsent = tx(client, session, 0, "555)")
      val txCorrectSession0 = tx(client, session, 1, "(555)")

      val txRightBracketAbsent = tx(client, session+1, 0, "(555")
      val txCorrectSession1 = tx(client, session+1, 1, "(555)")

      val txNoBracket = tx(client, session+2, 0, "555")
      val txCorrectSession2 = tx(client, session+2, 1, "(555)")

      sendCommit()
      sendCommit()

      sendDeliverTx(txLeftBracketAbsent)
      sendDeliverTx(txCorrectSession0) shouldBe (CodeType.BAD, ClientInfoMessages.SessionAlreadyClosed)

      sendCommit()
      sendCommit()

      sendDeliverTx(txRightBracketAbsent)
      sendDeliverTx(txCorrectSession1) shouldBe (CodeType.BAD, ClientInfoMessages.SessionAlreadyClosed)

      sendCommit()
      sendCommit()

      sendDeliverTx(txNoBracket)
      sendDeliverTx(txCorrectSession2) shouldBe (CodeType.BAD, ClientInfoMessages.SessionAlreadyClosed)

      sendCommit()
      sendCommit()

      sendQuery(s"@meta/${client.id}/$session/0/result") shouldBe
        Right(Error("WrongPayloadArgument", "Wrong payload argument=555)").toStoreValue)

      sendQuery(s"@meta/${client.id}/${session+1}/0/result") shouldBe
        Right(Error("WrongPayloadArgument", "Wrong payload argument=(555").toStoreValue)

      sendQuery(s"@meta/${client.id}/${session+2}/0/result") shouldBe
        Right(Error("WrongPayloadArgument", "Wrong payload argument=555").toStoreValue)
    }

    "invoke session txs in session counter order" in {
      sendCommit()
      sendCommit()

      sendDeliverTx(tx0)
      sendDeliverTx(tx2)
      sendDeliverTx(tx3)
      sendCommit()
      sendCommit()

      sendQuery(tx0Result) shouldBe Right(Computed(littleEndian4ByteHex(1)).toStoreValue)
      sendQuery(tx1Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))
      sendQuery(tx2Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))
      sendQuery(tx3Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))

      sendDeliverTx(tx1)
      sendCommit()
      sendCommit()

      sendQuery(tx0Result) shouldBe Right(Computed(littleEndian4ByteHex(1)).toStoreValue)
      sendQuery(tx1Result) shouldBe Right(Computed(littleEndian4ByteHex(140)).toStoreValue)
      sendQuery(tx2Result) shouldBe Right(Computed(littleEndian4ByteHex(2)).toStoreValue)
      sendQuery(tx3Result) shouldBe Right(Computed(littleEndian4ByteHex(3)).toStoreValue)
    }

    "ignore incorrectly signed tx" in {
      sendCommit()
      sendCommit()

      val txWithWrongSignature = tx(client, session, 0, "()", "bad_signature")
      sendCheckTx(txWithWrongSignature) shouldBe (CodeType.BAD, ClientInfoMessages.InvalidSignature)
      sendDeliverTx(txWithWrongSignature) shouldBe (CodeType.BAD, ClientInfoMessages.InvalidSignature)
    }

    "ignore duplicated tx" in {
      sendCommit()
      sendCommit()

      sendCheckTx(tx0) shouldBe (CodeType.OK, ClientInfoMessages.SuccessfulTxResponse)
      sendDeliverTx(tx0) shouldBe (CodeType.OK, ClientInfoMessages.SuccessfulTxResponse)
      // Mempool state updated only on commit!
      sendCheckTx(tx0) shouldBe (CodeType.OK, ClientInfoMessages.SuccessfulTxResponse)
      sendCommit()

      sendCheckTx(tx0) shouldBe (CodeType.BAD, ClientInfoMessages.DuplicatedTransaction)
      sendDeliverTx(tx0) shouldBe (CodeType.BAD, ClientInfoMessages.DuplicatedTransaction)
    }

    "process Query method correctly" in {
      sendDeliverTx(tx0)
      sendQuery(tx0Result) shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.QueryStateIsNotReadyYet))

      sendCommit()
      sendQuery(tx0Result) shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.QueryStateIsNotReadyYet))

      sendCommit()
      sendQuery("") shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.InvalidQueryPath))
      sendQuery("/a/b/") shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.InvalidQueryPath))
      sendQuery("/a/b") shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.InvalidQueryPath))
      sendQuery("a/b/") shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.InvalidQueryPath))
      sendQuery("a//b") shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.InvalidQueryPath))
      sendQuery(tx0Result, 2) shouldBe Left((QueryCodeType.Bad, ClientInfoMessages.RequestingCustomHeightIsForbidden))
      sendQuery(tx0Result) shouldBe Right(Computed(littleEndian4ByteHex(1)).toStoreValue)
    }

    "change session summary if session explicitly closed" in {
      sendCommit()
      sendCommit()

      sendDeliverTx(tx0)
      sendDeliverTx(tx1)
      sendDeliverTx(tx2)
      sendDeliverTx(tx3)
      sendDeliverTx(tx(client, session, 4, "@closeSession"))
      sendCommit()
      sendCommit()

      sendQuery(s"@meta/${client.id}/$session/4/status") shouldBe Right("sessionClosed")
      sendQuery(s"@meta/${client.id}/$session/@sessionSummary") shouldBe
        Right("{\"status\":{\"ExplicitlyClosed\":{}},\"invokedTxsCount\":5,\"lastTxCounter\":5}")
    }

    "not accept new txs if session failed" in {
      sendCommit()
      sendCommit()

      sendDeliverTx(tx0Failed)
      sendDeliverTx(tx1) shouldBe (CodeType.BAD, ClientInfoMessages.SessionAlreadyClosed)

      sendCommit()
      sendCommit()

      sendQuery(s"@meta/${client.id}/$session/0/result") shouldBe
        Right(Error("NoSuchModuleError", "Unable to find a module with the name=WrongModuleName").toStoreValue)
    }

    "not invoke dependent txs if required failed when order in not correct" in {
      sendCommit()
      sendCommit()

      sendDeliverTx(tx1)
      sendDeliverTx(tx2)
      sendDeliverTx(tx3)
      sendDeliverTx(tx0Failed)

      sendCommit()
      sendCommit()

      sendQuery(s"@meta/${client.id}/$session/0/result") shouldBe
        Right(Error("NoSuchModuleError", "Unable to find a module with the name=WrongModuleName").toStoreValue)
      sendQuery(s"@meta/${client.id}/$session/0/status") shouldBe Right("error")
      sendQuery(tx1Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))
      sendQuery(tx3Result) shouldBe Left((QueryCodeType.NotReady, ClientInfoMessages.ResultIsNotReadyYet))
    }

    "expire session after expiration period elapsed" in {
      sendCommit()
      sendCommit()

      val firstSession = "000001"
      val secondSession = "000002"
      val thirdSession = "000003"
      sendDeliverTx(tx(client, firstSession, 0, "()"))
      sendDeliverTx(tx(client, secondSession, 0, "()"))
      for (i <- 0 to 5)
        sendDeliverTx(tx(client, thirdSession, i, "()"))
      sendDeliverTx(tx(client, thirdSession, 6, "@closeSession"))
      sendCommit()
      sendCommit()

      sendQuery(s"@meta/${client.id}/$firstSession/@sessionSummary") shouldBe
        Right("{\"status\":{\"Expired\":{}},\"invokedTxsCount\":1,\"lastTxCounter\":1}")
      sendQuery(s"@meta/${client.id}/$secondSession/@sessionSummary") shouldBe
        Right("{\"status\":{\"Active\":{}},\"invokedTxsCount\":1,\"lastTxCounter\":2}")
      sendQuery(s"@meta/${client.id}/$thirdSession/@sessionSummary") shouldBe
        Right("{\"status\":{\"ExplicitlyClosed\":{}},\"invokedTxsCount\":7,\"lastTxCounter\":9}")
    }
  }
}
