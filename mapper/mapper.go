package mapper

import (
    "encoding/json"
    "fmt"
)

type ClientMessage struct {
    MsgType string `json:"type"`
    Msg     string `json:"msg"`
    Sender  string `json:"sender"`
}

var dict string = `{
	"mapper": {
		"height": 200,
		"width": 200
	}
}`

func main() {
    fmt.Println(dict)

    type Dimension struct {
        Height int
        Width int
    }

    type MapObj struct {
        Map Dimension
    }

    jo := MapObj{}
    json.Unmarshal([]byte(dict), &jo)
    fmt.Println(jo)

}
