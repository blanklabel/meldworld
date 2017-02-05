package main

import (
	"encoding/json"
	"fmt"
)

type ClientMessage struct {
	MsgType string `json:"type"`
	Msg     string `json:"msg"`
	Sender  string `json:"sender"`
}

var dict string = `{"entities":[
    {"name": "guy",
      "statuses": [],
      "full_hp": 27,
      "c_hp": 27,
      "phy_def": 13,
      "phy_atk": 13,
      "speed": 1,
      "coordinates": {"x": 0, "y": 1},
      "destination": {"x": 2, "y": 3}
    },

    {"name": "player4",
      "statuses": [],
      "full_hp": 2,
      "c_hp": 1,
      "phy_def": 2,
      "phy_atk": 2,
      "speed": 1,
      "coordinates": {"x": 0, "y": 1},
      "destination": {"x": 6, "y": 7}
    }
  ]}`

func main() {
	fmt.Println(dict)
	//d := websocket.Dialer{}
	//
	//wsHeaders := http.Header{
	//"Origin": {"http://localhost:8080"},
	//"Sec-WebSocket-Extensions": {"permessage-deflate; client_max_window_bits," +
	//	" x-webkit-deflate-frame"},
	//}
	//
	//wsConn, resp, err := d.Dial("ws://localhost:8080/game", wsHeaders)
	//fmt.Println(wsConn, resp, err)
	//b := []byte("move and groove")
	//wsConn.WriteMessage(1, b)
	//cmsg := &ClientMessage{}
	//for {
	//    wsConn.ReadJSON(cmsg)
	//    fmt.Println(cmsg.Sender, cmsg.Msg)
	//}
	var dat map[string]interface{}
	json.Unmarshal([]byte(dict), &dat)
	fmt.Println(dat)

	type Cords struct {
		X, Y int
	}

	type Entity struct {
		Name        string
		Full_hp     int
		C_hp        int
		Phy_def     int
		Phy_atk     int
		Speed       int
		Coordinates Cords
		Destination Cords
	}

	type EntityObj struct {
		Entities []Entity
	}

	jo := EntityObj{}
	json.Unmarshal([]byte(dict), &jo)
	fmt.Println(jo)

	e := Entity{"Bob", 1, 2, 3,
		4, 5, Cords{1, 2},
		Cords{4, 5}}

	jo.Entities = append(jo.Entities, e)

	q, _ := json.Marshal(jo)
	fmt.Println(q)

	nutmonkey := EntityObj{}
	json.Unmarshal(q, &nutmonkey)

	fmt.Println(nutmonkey)

}
