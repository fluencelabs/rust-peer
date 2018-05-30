package fluence.client

import monix.eval.Task
import monix.execution.Scheduler
import org.scalajs.dom.document
import org.scalajs.dom.html.{Div, Input}
import org.scalajs.dom.raw.HTMLElement

object GetElement extends slogging.LazyLogging {

  /**
   * Add element with `get` logic.
   *
   * @param el Append get element to this element.
   * @param action Action, that will be processed on button click or by pressing `enter` key
   * @param resultField Field, that will be show the result of action.
   */
  def addGetElement(el: HTMLElement, action: String ⇒ Task[Option[String]], resultField: Input)(
    implicit scheduler: Scheduler
  ): Unit = {

    val div = document.createElement("div").asInstanceOf[Div]

    val getInput = document.createElement("input").asInstanceOf[Input]
    getInput.`type` = "input"
    getInput.name = "put"

    val getButton = document.createElement("input").asInstanceOf[Input]
    getButton.`type` = "submit"
    getButton.value = "Get"

    div.appendChild(document.createElement("br"))
    div.appendChild(getInput)
    div.appendChild(getButton)
    div.appendChild(document.createElement("br"))

    def getAction = {
      if (!getButton.disabled) {
        getButton.disabled = true
        val key = getInput.value
        logger.info(s"Get key: $key")
        val t = for {
          res ← action(key).map(Utils.prettyResult)
        } yield {
          val printResult = s"Get operation success. Value: $res"
          logger.info(printResult)
          resultField.value = printResult
          getInput.value = ""
        }
        t.runAsync.onComplete(_ ⇒ getButton.disabled = false)
      }
    }

    getButton.onclick = mouseEvent ⇒ {
      getAction
    }

    getInput.onkeypress = keyboardEvent ⇒ {
      if (keyboardEvent.charCode == 13) {
        getAction
      }
    }

    el.appendChild(div)
  }
}
