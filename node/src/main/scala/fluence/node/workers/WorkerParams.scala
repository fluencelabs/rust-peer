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

package fluence.node.workers
import java.nio.file.Path

import fluence.node.docker.DockerImage
import fluence.node.eth.state.WorkerPeer
import fluence.node.eth.state.App
import fluence.node.workers.tendermint.config.ConfigTemplate

/**
 * Worker container's params
 */
case class WorkerParams(
  app: App,
  dataPath: Path,
  vmCodePath: Path,
  masterNodeContainerId: Option[String],
  image: DockerImage,
  configTemplate: ConfigTemplate
) {
  def appId: Long = app.id

  def currentWorker: WorkerPeer = app.cluster.currentWorker

  override def toString =
    s"(worker ${currentWorker.index} with RPC port ${currentWorker.rpcPort} for app $appId)"

}
